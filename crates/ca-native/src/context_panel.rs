//! The Context view (`/context`, the ring's What fills it, Agent: Show the Context): what fills the model's window
//! in the next request, each part with its tokens and share opening to its pieces (each tool's definition, each
//! message), the room kept for the answer and what is free, how it was counted, and what could be freed (Compact
//! now). Also the status bar's Context item, `USED / LIMIT`, amber at the threshold, which opens the compaction
//! settings: when the session compacts (at a share of the window, only when a request would not fit, or as the
//! Arena account says) and how much of the recent part it keeps, saved with `compact_at`.
//!
//! The numbers are the ring's (`agent::ring::Context`): this file only draws them.

use floem::event::{Event, EventListener};
use floem::keyboard::{Key, NamedKey};
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_effect, create_memo, create_rw_signal};
use floem::style::{CursorStyle, TextOverflow};
use floem::text::Weight;
use floem::views::{
    Decorators, container, dyn_container, empty, h_stack, label, scroll, text_input, tooltip, v_stack,
    v_stack_from_iter,
};
use floem::{AnyView, IntoView, View};
use serde_json::{Value, json};

use crate::agent::ring::{self, Context, Detail, Window, kind_color, short};
use crate::commands;
use crate::icons::icon;
use crate::layers::{self, Place};
use crate::state::{Send, Ui};
use crate::statusbar::{self, Side};
use crate::widgets::{Theme, action, button, toast};

/// Whether the view is in the window.
pub const READY: bool = true;

/// The defaults and bounds of the compaction settings, as the old Code Arena's.
pub const DEFAULT_AT: u64 = 80;
pub const DEFAULT_TARGET: u64 = 25;

/// When the session compacts, as the settings' radio says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum When {
    Share,
    Off,
    Account,
}

/// What the settings' fields say, checked: what `compact_at` is sent (`at`: a share, 0 for only when needed, or
/// `"account"`; `target`), else why not.
pub fn validate(when: When, at: &str, target: &str, account: Option<i64>) -> Result<(Value, u64), String> {
    let a = match when {
        When::Off => 0,
        When::Account => account.map_or(DEFAULT_AT, |n| n.clamp(0, 95) as u64),
        When::Share => match at.trim().parse::<u64>() {
            Ok(n) if (20..=95).contains(&n) => n,
            _ => return Err("The threshold is from 20% to 95% of the window.".into()),
        },
    };
    // Only when needed compacts near the window's end: what is kept stays as far under the highest threshold.
    let ceiling = if a == 0 { 95 } else { a } - 10;
    let t = match target.trim().parse::<u64>() {
        Ok(t) if (5..=ceiling).contains(&t) => t,
        _ => return Err(format!("What is kept is from 5% to {ceiling}% (10 points under the threshold).")),
    };
    let sent = if when == When::Account { json!("account") } else { json!(a) };
    Ok((sent, t))
}

/// What the toast says once the settings are saved.
pub fn saved_line(at: u64, target: u64) -> String {
    if at == 0 {
        format!("The session compacts only when a request would not fit, keeping {target}%.")
    } else {
        format!("The session compacts at {at}% of the window, keeping {target}%.")
    }
}

/// The account's radio row: what the account says, or what is read when it said nothing.
pub fn account_label(account: Option<i64>) -> String {
    match account {
        None => "As my Arena account says (Arena did not say: 80%)".into(),
        Some(n) if n > 0 => format!("As my Arena account says ({n}%)"),
        Some(_) => "As my Arena account says (only when needed)".into(),
    }
}

/// The compaction settings while open.
#[derive(Clone, Copy)]
struct Settings {
    open: RwSignal<bool>,
    when: RwSignal<When>,
    at: RwSignal<String>,
    target: RwSignal<String>,
    error: RwSignal<Option<String>>,
    saving: RwSignal<bool>,
}

impl crate::ext::State for Settings {
    fn new(ui: Ui) -> Settings {
        Settings {
            open: ui.scope.create_rw_signal(false),
            when: ui.scope.create_rw_signal(When::Share),
            at: ui.scope.create_rw_signal(String::new()),
            target: ui.scope.create_rw_signal(String::new()),
            error: ui.scope.create_rw_signal(None),
            saving: ui.scope.create_rw_signal(false),
        }
    }
}

/// Opens the compaction settings, filled from the window as it is.
pub fn open_settings(ui: Ui, send: &Send) {
    let st = ui.state::<Settings>();
    let w = ui.state::<Context>().window.get_untracked();
    let when = match (w.source.as_str(), w.at) {
        ("account", _) => When::Account,
        (_, 0) => When::Off,
        _ => When::Share,
    };
    st.when.set(when);
    st.at.set(if w.at > 0 { w.at } else { DEFAULT_AT }.to_string());
    st.target.set(if w.target > 0 { w.target } else { DEFAULT_TARGET }.to_string());
    st.error.set(None);
    st.saving.set(false);
    st.open.set(true);
    ring::ask(send);
}

fn save(ui: Ui, send: &Send) {
    let st = ui.state::<Settings>();
    let account = ui.state::<Context>().window.with_untracked(|w| w.account);
    match validate(st.when.get_untracked(), &st.at.get_untracked(), &st.target.get_untracked(), account) {
        Ok((at, target)) => {
            st.error.set(None);
            st.saving.set(true);
            send(json!({"type": "compact_at", "at": at, "target": target}));
        }
        Err(e) => st.error.set(Some(e)),
    }
}

/// A line of small grey words, wrapped.
fn note(t: Theme, text: impl Fn() -> String + 'static) -> impl IntoView {
    label(text).style(move |s| s.font_size(12.0).line_height(1.4).color(t.get().desc).text_overflow(TextOverflow::Wrap))
}

/// A header of a section of the view.
fn heading(text: &'static str) -> impl IntoView {
    label(move || text).style(|s| s.font_size(11.0).font_weight(Weight::BOLD).margin_top(8.0).selectable(false))
}

/// One part of the window: its row (a chevron, its swatch, its name, tokens and share), opening to its pieces.
fn part(ui: Ui, p: ring::Part, size: u64) -> AnyView {
    let t = ui.colors;
    let open = create_rw_signal(false);
    let color = kind_color(&t.get_untracked(), &p.kind);
    let (kind, tokens, share) = (p.kind.clone(), p.tokens, p.tokens * 100 / size.max(1));
    let has = !p.pieces.is_empty();
    let row = h_stack((
        icon("chevron-right", 16.0).style(move |s| {
            s.apply_if(open.get(), |s| s.hide()).apply_if(!has, |s| s.color(floem::peniko::Color::TRANSPARENT))
        }),
        icon("chevron-down", 16.0).style(move |s| s.apply_if(!open.get(), |s| s.hide())),
        empty().style(move |s| s.width(10.0).height(10.0).border_radius(2.0).background(color).flex_shrink(0.0_f32)),
        label(move || kind.clone()).style(|s| s.flex_grow(1.0_f32).min_width(0.0).text_ellipsis()),
        label(move || short(tokens)).style(move |s| s.color(t.get().desc)),
        label(move || format!("{share}%")).style(move |s| s.width(36.0).justify_end().color(t.get().desc)),
    ))
    .on_click_stop(move |_| {
        if has {
            open.update(|o| *o = !*o);
        }
    })
    .style(move |s| {
        let c = t.get();
        s.items_center()
            .gap(6.0)
            .height(22.0)
            .padding_right(8.0)
            .width_full()
            .apply_if(has, |s| s.cursor(CursorStyle::Pointer))
            .hover(|s| s.background(c.list_hover))
    });
    let pieces = p.pieces;
    let list = dyn_container(
        move || open.get(),
        move |shown| {
            if !shown {
                return empty().into_any();
            }
            v_stack_from_iter(pieces.iter().map(|piece| {
                let (name, tokens, more) = (piece.name.clone(), piece.tokens, piece.more);
                let tip = name.clone();
                let row = h_stack((
                    label(move || name.clone()).style(move |s| {
                        s.flex_grow(1.0_f32).min_width(0.0).text_ellipsis().apply_if(more, |s| s.color(t.get().desc))
                    }),
                    label(move || short(tokens)).style(move |s| s.color(t.get().desc)),
                ))
                .style(|s| s.items_center().gap(8.0).height(22.0).padding_left(38.0).padding_right(8.0).width_full());
                // The whole name, for one cut short.
                tooltip(row, move || {
                    let tip = tip.clone();
                    label(move || tip.clone())
                })
                .style(|s| s.width_full())
                .into_any()
            }))
            .style(|s| s.width_full())
            .into_any()
        },
    )
    .style(|s| s.width_full());
    v_stack((row, list)).style(|s| s.width_full()).into_any()
}

/// A plain row of the list (the room kept for the answer, what is free).
fn plain_row(t: Theme, color: floem::peniko::Color, name: &'static str, tokens: u64, size: u64) -> impl IntoView {
    let share = tokens * 100 / size.max(1);
    h_stack((
        empty().style(|s| s.width(16.0)),
        empty().style(move |s| s.width(10.0).height(10.0).border_radius(2.0).background(color).flex_shrink(0.0_f32)),
        label(move || name).style(|s| s.flex_grow(1.0_f32).min_width(0.0)),
        label(move || short(tokens)).style(move |s| s.color(t.get().desc)),
        label(move || format!("{share}%")).style(move |s| s.width(36.0).justify_end().color(t.get().desc)),
    ))
    .style(|s| s.items_center().gap(6.0).height(22.0).padding_right(8.0).width_full())
}

/// The view's content for the window and detail as they are now.
fn content(ui: Ui, send: Send, w: Window, d: Option<Detail>, width: f64) -> AnyView {
    let t = ui.colors;
    let Some(d) = d else {
        let said = if w.size == 0 {
            "What fills the window is shown once the session has started."
        } else {
            "Reading what fills the window…"
        };
        return note(t, move || said.into()).style(|s| s.padding(12.0)).into_any();
    };
    let c = t.get_untracked();
    let mut rows: Vec<AnyView> = vec![
        note(t, || "What fills the model's window in the next request: each part, its pieces and their tokens.".into())
            .into_any(),
        label({
            let line = ring::use_line(&w);
            move || line.clone()
        })
        .style(|s| s.font_size(12.0).text_overflow(TextOverflow::Wrap))
        .into_any(),
    ];
    if let Some(over) = ring::over_line(&w) {
        rows.push(note(t, move || over.clone()).style(move |s| s.color(t.get().warning)).into_any());
    }
    rows.push(ring::bar(t, &w, Some(&d), (width - 24.0).max(60.0)));
    let mut list: Vec<AnyView> = d.parts.iter().filter(|p| p.tokens > 0).map(|p| part(ui, p.clone(), w.size)).collect();
    if w.reserve > 0 {
        list.push(plain_row(t, c.charts_lines, "Kept for the answer", w.reserve, w.size).into_any());
    }
    list.push(plain_row(t, c.input_bg, "Free", w.size.saturating_sub(w.used + w.reserve), w.size).into_any());
    rows.push(v_stack_from_iter(list).style(|s| s.width_full().margin_left(-8.0)).into_any());
    let counted = if d.counted == "arena" {
        "Counted by Arena with the model's tokenizer."
    } else {
        "Estimated by Code Arena from the characters sent, at the rate the model's own counts have shown."
    };
    rows.push(note(t, move || counted.into()).into_any());
    rows.push(heading("COULD BE FREED").into_any());
    if d.could_free.is_empty() {
        rows.push(note(t, || "Nothing to free now.".into()).into_any());
    }
    for (group, tokens) in d.could_free.clone() {
        let send = send.clone();
        let name = if group == "compact" { "The older messages, as a summary".to_string() } else { group.clone() };
        rows.push(
            h_stack((
                label(move || name.clone()).style(|s| s.flex_grow(1.0_f32).min_width(0.0).text_ellipsis()),
                label(move || short(tokens)).style(move |s| s.color(t.get().desc)),
                button(t, "Compact now", false, move || ring::compact_now(ui, &send)),
            ))
            .style(|s| s.items_center().gap(8.0).width_full())
            .into_any(),
        );
    }
    rows.push(
        note(t, move || {
            let running = if ui.running.get() { "A turn runs: this is the window as it was before it. " } else { "" };
            format!(
                "{running}A big result is kept aside (the model reads its start and end, and read_result the rest); before compacting, earlier results are cleared to a note."
            )
        })
        .style(|s| s.margin_top(8.0))
        .into_any(),
    );
    v_stack_from_iter(rows).style(|s| s.gap(8.0).padding(12.0).width_full().font_size(13.0)).into_any()
}

/// The view's own content.
pub fn view(ui: Ui, send: Send) -> AnyView {
    let st = ui.state::<Context>();
    let width = create_rw_signal(300.0_f64);
    st.views.update(|n| *n += 1);
    ring::ask(&send);
    let what = create_memo(move |_| (st.window.get(), st.detail.get(), width.get()));
    let body = dyn_container(move || what.get(), move |(w, d, wide)| content(ui, send.clone(), w, d, wide))
        .style(|s| s.width_full());
    scroll(body)
        .on_resize(move |r| {
            let w = r.width().round();
            if (width.get_untracked() - w).abs() >= 1.0 {
                width.set(w);
            }
        })
        .on_cleanup(move || st.views.update(|n| *n = n.saturating_sub(1)))
        .style(|s| s.size_full())
        .into_any()
}

/// The actions in its title: read it again, compact now.
pub fn actions(ui: Ui, send: Send) -> Option<AnyView> {
    let t = ui.colors;
    let again = send.clone();
    Some(
        h_stack((
            tooltip(action(t, "refresh", move || ring::ask(&again)), || label(|| "Refresh")),
            tooltip(action(t, "fold", move || ring::compact_now(ui, &send)), || label(|| "Compact Now")),
        ))
        .into_any(),
    )
}

/// The status bar's item: `USED / LIMIT`, amber from the threshold on.
fn item(ui: Ui, send: Send) -> AnyView {
    let t = ui.colors;
    let st = ui.state::<Context>();
    let shown = create_memo(move |_| st.window.with(|w| (w.used, w.size, w.percent() >= w.warn_at())));
    let tip = move || {
        let w = st.window.get();
        let when = if w.at > 0 { format!("compacts at {}%", w.at) } else { "compacts only when a request would not fit".into() };
        format!("Context: {} of {} tokens ({}%), {when}", short(w.used), short(w.size), w.percent())
    };
    let inner = h_stack((
        icon("dashboard", 14.0),
        label(move || {
            let (used, size, _) = shown.get();
            format!("{} / {}", short(used), short(size))
        }),
    ))
    .on_click_stop(move |_| open_settings(ui, &send))
    .style(move |s| {
        let c = t.get();
        let (_, size, warn) = shown.get();
        statusbar::item(t, s)
            .cursor(CursorStyle::Pointer)
            .apply_if(warn, |s| s.background(c.status_warning_bg).color(c.status_warning_fg))
            .apply_if(size == 0, |s| s.hide())
    });
    tooltip(inner, move || label(tip)).style(|s| s.height_full()).into_any()
}

/// A radio row of the settings.
fn radio(ui: Ui, value: When, text: impl Fn() -> String + 'static) -> impl IntoView {
    let t = ui.colors;
    let st = ui.state::<Settings>();
    h_stack((
        icon("circle-large-filled", 16.0).style(move |s| s.apply_if(st.when.get() != value, |s| s.hide())),
        icon("circle-large", 16.0).style(move |s| s.apply_if(st.when.get() == value, |s| s.hide())),
        label(text),
    ))
    .on_click_stop(move |_| {
        if st.when.get_untracked() != value {
            st.when.set(value);
        }
    })
    .style(move |s| s.items_center().gap(8.0).cursor(CursorStyle::Pointer).color(t.get().fg))
}

/// A number field of the settings, with its label and hint under it.
fn field(ui: Ui, send: Send, name: &'static str, hint: &'static str, value: RwSignal<String>) -> impl IntoView {
    let t = ui.colors;
    let input = text_input(value)
        .on_event_cont(EventListener::KeyDown, move |e| {
            if let Event::KeyDown(k) = e
                && k.key.logical_key == Key::Named(NamedKey::Enter)
            {
                save(ui, &send);
            }
        })
        .style(move |s| {
            let c = t.get();
            s.width_full()
                .height(26.0)
                .padding_horiz(6.0)
                .border(1.0)
                .border_radius(2.0)
                .border_color(c.input_border)
                .background(c.input_bg)
                .color(c.input_fg)
                .focus(|s| s.border_color(c.focus))
        });
    v_stack((
        label(move || name).style(|s| s.font_size(12.0).font_weight(Weight::SEMIBOLD)),
        input,
        label(move || hint).style(move |s| s.font_size(12.0).color(t.get().desc)),
    ))
    .style(|s| s.gap(4.0).flex_grow(1.0_f32).flex_basis(0.0).min_width(0.0))
}

/// The compaction settings' box.
fn settings_box(ui: Ui, send: Send) -> AnyView {
    let t = ui.colors;
    let st = ui.state::<Settings>();
    let ctx = ui.state::<Context>();
    let now = create_memo(move |_| ctx.window.get());
    let detail = create_memo(move |_| (ctx.window.get(), ctx.detail.get()));
    let description = move || {
        let w = now.get();
        let when = if w.at > 0 { format!("at {}%", w.at) } else { "only when a request would not fit".into() };
        format!(
            "When the conversation reaches the threshold, its older part is summarized and the recent part kept whole; and always before a request would not fit. Now: {when} ({}).",
            ring::who(&w.source)
        )
    };
    let parts = dyn_container(
        move || detail.get(),
        move |(w, d)| match d {
            Some(d) => v_stack((ring::bar(t, &w, Some(&d), 408.0), ring::kinds(t, &w, &d))).style(|s| s.gap(8.0)).into_any(),
            None => ring::bar(t, &w, None, 408.0),
        },
    );
    let (s1, s2, s3) = (send.clone(), send.clone(), send);
    let fields = h_stack((
        field(ui, s1, "Compact at (% of the window)", "Default 80%", st.at)
            .style(move |s| s.apply_if(st.when.get() != When::Share, |s| s.hide())),
        field(ui, s2, "Keep the recent part within (%)", "Default 25%", st.target),
    ))
    .style(|s| s.gap(16.0).width_full());
    let error = label(move || st.error.get().unwrap_or_default())
        .style(move |s| s.font_size(13.0).color(t.get().error).apply_if(st.error.with(Option::is_none), |s| s.hide()));
    let defaults = button(t, "Defaults", false, move || {
        st.when.set(When::Share);
        st.at.set(DEFAULT_AT.to_string());
        st.target.set(DEFAULT_TARGET.to_string());
        st.error.set(None);
    });
    let buttons = h_stack((
        defaults,
        empty().style(|s| s.flex_grow(1.0_f32)),
        button(t, "Cancel", false, move || st.open.set(false)),
        button(t, "Save", true, move || save(ui, &s3)),
    ))
    .style(|s| s.gap(8.0).width_full().margin_top(4.0));
    v_stack((
        label(|| "Context").style(|s| s.font_size(15.0).font_weight(Weight::SEMIBOLD)),
        label(description).style(|s| s.line_height(1.4).text_overflow(TextOverflow::Wrap)),
        note(t, move || ring::use_line(&now.get())),
        parts,
        v_stack((
            radio(ui, When::Share, || "At a share of the window".into()),
            radio(ui, When::Off, || "Only when a request would not fit".into()),
            radio(ui, When::Account, move || account_label(now.get().account)),
        ))
        .style(|s| s.gap(6.0)),
        fields,
        error,
        buttons,
    ))
    .on_event_stop(EventListener::PointerDown, |_| {})
    .style(move |s| {
        let c = t.get();
        s.width(450.0)
            .gap(12.0)
            .padding(20.0)
            .border(1.0)
            .border_color(c.widget_border)
            .border_radius(6.0)
            .background(c.widget)
            .color(c.fg)
            .font_size(13.0)
            .box_shadow_blur(16.0)
            .box_shadow_color(c.shadow)
    })
    .into_any()
}

/// The settings' layer: the window dimmed under the box while open; Escape or a press outside closes it.
fn settings_layer(ui: Ui, send: Send) -> AnyView {
    let st = ui.state::<Settings>();
    let body = dyn_container(
        move || st.open.get(),
        move |open| if open { settings_box(ui, send.clone()) } else { empty().into_any() },
    );
    let layer = container(body).keyboard_navigable();
    let id = layer.id();
    create_effect(move |_| {
        if st.open.get() {
            id.request_focus();
        }
    });
    layer
        .on_event_stop(EventListener::PointerDown, move |_| st.open.set(false))
        .on_event_cont(EventListener::KeyDown, move |e| {
            if let Event::KeyDown(k) = e
                && k.key.logical_key == Key::Named(NamedKey::Escape)
            {
                st.open.set(false);
            }
        })
        .style(move |s| {
            s.absolute()
                .inset(0.0)
                .items_center()
                .justify_center()
                .background(floem::peniko::Color::rgba8(0, 0, 0, 0x4D))
                .apply_if(!st.open.get(), |s| s.hide())
        })
        .into_any()
}

/// What it adds to the window's registries, once at start: the status bar's item, the settings' layer, and the
/// commands.
pub fn register(_ui: Ui, _send: &Send) {
    statusbar::add("status.context", Side::Right, 280, item);
    layers::add(Place::Over, "context.settings", 30, settings_layer);
    commands::add(commands::command("codeArena.agent.showContext", "Agent", "Show the Context", |ui, send| {
        ring::show_view(ui, send)
    }));
    commands::add(commands::command("codeArena.agent.compact", "Agent", "Compact Now", |ui, send| {
        ring::compact_now(ui, send)
    }));
    commands::add(commands::command("codeArena.agent.compactionSettings", "Agent", "Compaction Settings…", |ui, send| {
        open_settings(ui, send)
    }));
}

/// The session's messages that are this part's, taken in: a threshold saved closes the settings, one refused says
/// why in them. Never taken away from the parts after it.
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    let st = ui.state::<Settings>();
    match m["type"].as_str().unwrap_or("") {
        "compact_set" if st.saving.get_untracked() => {
            st.saving.set(false);
            st.open.set(false);
            let (at, target) = (m["at"].as_u64().unwrap_or(0), m["target"].as_u64().unwrap_or(0));
            toast::info(ui, &saved_line(at, target));
        }
        "compact_refused" if st.saving.get_untracked() => {
            st.saving.set(false);
            st.error.set(Some(m["error"].as_str().unwrap_or("The settings were not saved.").to_string()));
        }
        _ => {}
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_settings_are_checked_as_the_old_code_arena_checks_them() {
        assert_eq!(validate(When::Share, "80", "25", None), Ok((json!(80), 25)));
        assert_eq!(validate(When::Off, "", "85", None), Ok((json!(0), 85)));
        assert_eq!(validate(When::Account, "", "25", Some(60)), Ok((json!("account"), 25)));
        assert_eq!(validate(When::Share, "19", "5", None), Err("The threshold is from 20% to 95% of the window.".into()));
        assert_eq!(validate(When::Share, "eighty", "5", None).is_err(), true);
        assert_eq!(
            validate(When::Share, "30", "25", None),
            Err("What is kept is from 5% to 20% (10 points under the threshold).".into())
        );
        assert_eq!(
            validate(When::Account, "", "75", None),
            Err("What is kept is from 5% to 70% (10 points under the threshold).".into()),
            "the account said nothing: 80%"
        );
    }

    #[test]
    fn the_toast_and_the_account_row_say_what_was_chosen() {
        assert_eq!(saved_line(80, 25), "The session compacts at 80% of the window, keeping 25%.");
        assert_eq!(saved_line(0, 30), "The session compacts only when a request would not fit, keeping 30%.");
        assert_eq!(account_label(None), "As my Arena account says (Arena did not say: 80%)");
        assert_eq!(account_label(Some(70)), "As my Arena account says (70%)");
        assert_eq!(account_label(Some(0)), "As my Arena account says (only when needed)");
    }
}
