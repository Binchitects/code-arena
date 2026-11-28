//! The context ring in the input's toolbar: how full the model's window is, as a ring and a share, red at the
//! threshold where the chat compacts (amber some way before). A click opens its popover over the box: the tokens
//! used of the window, a bar of what fills it by kind with the threshold's tick, each kind with the room kept for
//! the answer and what is free, the biggest messages, when it compacts and who set that, and What fills it (the
//! Context view) and Compact now.
//!
//! The numbers come in `state` and `status` (`context`, `compact`), with each step of a running turn (`step_end`'s
//! `context`, as the model counted it) and in `context_detail` (asked when the popover or the Context view opens,
//! and after a turn while one of them is open). The Context view (`context_panel.rs`) reads the same state.

use std::f64::consts::{FRAC_PI_2, TAU};

use floem::context::PaintCx;
use floem::kurbo::{Arc, Cap, Circle, Stroke};
use floem::peniko::Color;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_effect, create_memo};
use floem::style::CursorStyle;
use floem::text::Weight;
use floem::views::{Decorators, container, dyn_container, empty, h_stack, h_stack_from_iter, label, v_stack, v_stack_from_iter};
use floem::{AnyView, IntoView, View, ViewId};
use floem_renderer::Renderer as _;
use serde_json::{Value, json};

use crate::agent::slots::{self, Slot};
use crate::colors::Colors;
use crate::layers::{self, Place};
use crate::state::{Send, Ui};
use crate::widgets::{Theme, window_size};

/// How full the window is and when it compacts, as the session last said.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Window {
    pub used: u64,
    pub size: u64,
    /// Where it compacts, a share of the window (0: only when a request would not fit).
    pub at: u64,
    /// What a compaction keeps whole, a share of the window.
    pub target: u64,
    /// Kept free for the answer.
    pub reserve: u64,
    /// Who set the threshold: `settings`, `account`, `flag`, `env`, `default`.
    pub source: String,
    /// What the Arena account says (0: only when needed); none when it said nothing.
    pub account: Option<i64>,
    /// As the model counted it (a step of a turn); else Code Arena's estimate.
    pub counted: bool,
}

impl Window {
    /// The window with what a message says of it (`context`, `compact`); the rest as it was. Counted only when
    /// the model counted it (`step_end`).
    pub fn with(&self, m: &Value) -> Window {
        let mut w = self.clone();
        let c = &m["context"];
        if let Some(size) = c["size"].as_u64() {
            w.used = c["used"].as_u64().unwrap_or(0);
            w.size = size;
            w.at = c["compact_at"].as_u64().unwrap_or(0);
            w.counted = m["type"] == "step_end";
        }
        // A threshold changed from the window says it at its top.
        let k = if m["type"] == "compact_set" { m } else { &m["compact"] };
        if k.is_object() {
            w.at = k["at"].as_u64().unwrap_or(w.at);
            w.target = k["target"].as_u64().unwrap_or(w.target);
            w.reserve = k["reserve"].as_u64().unwrap_or(w.reserve);
            w.source = k["source"].as_str().unwrap_or(&w.source).to_string();
            w.account = k["account"].as_i64();
        }
        if let Some(r) = m["reserve"].as_u64() {
            w.reserve = r;
        }
        w
    }

    /// The share used, in percent (at most 100).
    pub fn percent(&self) -> u64 {
        if self.size == 0 { 0 } else { (self.used * 100 / self.size).min(100) }
    }

    /// The share where the ring turns red: the threshold, or 90% when it compacts only when needed.
    pub fn warn_at(&self) -> u64 {
        if self.at > 0 { self.at } else { 90 }
    }

    /// What it holds with the room for the answer is more than the window: the next message compacts first.
    pub fn over(&self) -> bool {
        self.size > 0 && self.used + self.reserve > self.size
    }

    pub fn level(&self) -> Level {
        let p = self.percent();
        if p >= self.warn_at() || self.over() {
            Level::Full
        } else if p + 15 >= self.warn_at() {
            Level::Near
        } else {
            Level::Fine
        }
    }
}

/// How near the threshold the window is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Fine,
    Near,
    Full,
}

/// One part of what fills the window (a kind) with its pieces.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub kind: String,
    pub tokens: u64,
    pub pieces: Vec<Piece>,
}

/// A piece of a part: a tool's definition, a message; `more` the row that says the rest.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub name: String,
    pub tokens: u64,
    pub more: bool,
}

/// One of the biggest messages.
#[derive(Clone, Debug, PartialEq)]
pub struct Big {
    pub role: String,
    pub tokens: u64,
    pub start: String,
}

/// What fills the next request, as `context_detail` said.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Detail {
    pub parts: Vec<Part>,
    pub reserve: u64,
    pub free: u64,
    /// `estimate` (Code Arena's, by characters) or `arena` (Arena's, with the model's tokenizer).
    pub counted: String,
    pub biggest: Vec<Big>,
    /// What could be freed: `(group, tokens)`; `compact` is compacting now.
    pub could_free: Vec<(String, u64)>,
}

impl Detail {
    pub fn of(m: &Value) -> Detail {
        let list = |v: &Value| v.as_array().cloned().unwrap_or_default();
        let text = |v: &Value| v.as_str().unwrap_or("").to_string();
        Detail {
            parts: list(&m["kinds"])
                .iter()
                .map(|k| Part {
                    kind: text(&k["kind"]),
                    tokens: k["tokens"].as_u64().unwrap_or(0),
                    pieces: list(&k["pieces"])
                        .iter()
                        .map(|p| Piece {
                            name: text(&p["name"]),
                            tokens: p["tokens"].as_u64().unwrap_or(0),
                            more: p["more"] == true,
                        })
                        .collect(),
                })
                .collect(),
            reserve: m["reserve"].as_u64().unwrap_or(0),
            free: m["free"].as_u64().unwrap_or(0),
            counted: text(&m["counted"]),
            biggest: list(&m["biggest"])
                .iter()
                .map(|b| Big { role: text(&b["role"]), tokens: b["tokens"].as_u64().unwrap_or(0), start: text(&b["start"]) })
                .collect(),
            could_free: list(&m["could_free"])
                .iter()
                .map(|f| (text(&f["group"]), f["tokens"].as_u64().unwrap_or(0)))
                .collect(),
        }
    }
}

/// Tokens as the old Code Arena writes them: 8.6K, 32.77K, 1.05M (thousands of a thousand, two decimals at most).
pub fn short(n: u64) -> String {
    let scaled = |v: f64, unit: &str| {
        let s = format!("{v:.2}");
        format!("{}{unit}", s.trim_end_matches('0').trim_end_matches('.'))
    };
    match n {
        0..1_000 => n.to_string(),
        1_000..1_000_000 => scaled(n as f64 / 1e3, "K"),
        _ => scaled(n as f64 / 1e6, "M"),
    }
}

/// "About 8.6K of 32.77K tokens (26%)", with "since compacting" and what the next answer does while estimated.
pub fn use_line(w: &Window) -> String {
    let about = if w.counted { "" } else { "About " };
    let then = if w.counted { "" } else { "; the next answer counts it" };
    format!("{about}{} of {} tokens ({}%){then}", short(w.used), short(w.size), w.percent())
}

/// What the line under it says when what it holds and the room for the answer pass the window.
pub fn over_line(w: &Window) -> Option<String> {
    w.over().then(|| {
        format!(
            "With the {} kept for the answer, this is more than the context: the next message compacts the chat first.",
            short(w.reserve)
        )
    })
}

/// Who set the threshold, in words.
pub fn who(source: &str) -> &'static str {
    match source {
        "settings" => "set in settings.json",
        "account" => "from your Arena account",
        "flag" => "set with --compact-at",
        "env" => "set in the environment",
        _ => "the default",
    }
}

/// When it compacts, in words: "Compacts at 80% (the default), and always before a message would not fit."
pub fn compacts(w: &Window) -> String {
    if w.at > 0 {
        format!("Compacts at {}% ({}), and always before a message would not fit.", w.at, who(&w.source))
    } else {
        format!("Compacts only when a message would not fit ({}).", who(&w.source))
    }
}

/// A biggest message's name, by its role.
pub fn role_name(role: &str) -> &'static str {
    match role {
        "you" => "Your message",
        "answer" => "An answer",
        "tool result" => "A tool's result",
        "note" => "A note",
        _ => "The instructions",
    }
}

/// A kind's colour in the bar: VS Code's chart colours.
pub fn kind_color(c: &Colors, kind: &str) -> Color {
    match kind {
        "Instructions" => c.charts_blue,
        "Tools" => c.charts_purple,
        "Your messages" => c.charts_green,
        "Answers" => c.charts_orange,
        "Tool results" => c.charts_yellow,
        _ => c.charts_red,
    }
}

/// The window's state for the ring, its popover, the status bar's item and the Context view.
#[derive(Clone, Copy)]
pub struct Context {
    pub window: RwSignal<Window>,
    pub detail: RwSignal<Option<Detail>>,
    /// The popover is open.
    pub open: RwSignal<bool>,
    /// The ring's button, the popover's anchor.
    pub ring: RwSignal<Option<ViewId>>,
    /// How many Context views are drawn: the detail is asked again after a turn while one is.
    pub views: RwSignal<u32>,
}

impl crate::ext::State for Context {
    fn new(ui: Ui) -> Context {
        Context {
            window: ui.scope.create_rw_signal(Window::default()),
            detail: ui.scope.create_rw_signal(None),
            open: ui.scope.create_rw_signal(false),
            ring: ui.scope.create_rw_signal(None),
            views: ui.scope.create_rw_signal(0),
        }
    }
}

/// Asks what fills the window (`context_detail`).
pub fn ask(send: &Send) {
    send(json!({"type": "context"}));
}

/// Compacts now: refused while an answer is written.
pub fn compact_now(ui: Ui, send: &Send) {
    if ui.running.get_untracked() {
        crate::widgets::toast::info(ui, "Compact when the answer is done.");
        return;
    }
    send(json!({"type": "compact"}));
}

/// The Context view shown, with what fills the window asked again.
pub fn show_view(ui: Ui, send: &Send) {
    crate::commands::run(ui, send, crate::layout::View::Context.command());
    ask(send);
}

/// What it adds to the window's registries, once at start: the ring before the send button, and its popover.
pub fn register(_ui: Ui, _send: &Send) {
    slots::add(Slot::BoxRight, "context.ring", 10, |ui, _| ring(ui).into_any());
    layers::add(Place::Over, "context.popover", 40, |ui, send| popover(ui, &send));
}

/// The session's messages that are this part's, taken in. Never taken away from the parts after it.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let st = ui.state::<Context>();
    let kind = m["type"].as_str().unwrap_or("");
    match kind {
        "state" | "status" | "step_end" | "compact_set" | "context_detail" => {
            let w = st.window.with_untracked(|w| w.with(m));
            if st.window.with_untracked(|was| *was != w) {
                st.window.set(w);
            }
            if kind == "context_detail" {
                let d = Some(Detail::of(m));
                if st.detail.with_untracked(|was| *was != d) {
                    st.detail.set(d);
                }
            }
        }
        "turn_end" if st.open.get_untracked() || st.views.get_untracked() > 0 => ask(send),
        _ => {}
    }
    false
}

/// The ring itself, painted: the window's whole as a faint circle, the share used as an arc from the top.
struct Ring {
    id: ViewId,
    ui: Ui,
}

impl View for Ring {
    fn id(&self) -> ViewId {
        self.id
    }

    fn paint(&mut self, cx: &mut PaintCx) {
        let w = self.ui.state::<Context>().window.get_untracked();
        let c = self.ui.colors.get_untracked();
        let r = self.id.get_content_rect();
        let center = r.center();
        let stroke = Stroke::new(2.0).with_caps(Cap::Round);
        cx.stroke(&Circle::new(center, 6.0), c.charts_lines, &stroke);
        let share = w.used as f64 / w.size.max(1) as f64;
        if share > 0.0 {
            let color = match w.level() {
                Level::Full => c.error,
                Level::Near => c.warning,
                Level::Fine => c.progress,
            };
            let arc = Arc::new(center, (6.0, 6.0), -FRAC_PI_2, TAU * share.min(1.0), 0.0);
            cx.stroke(&arc, color, &stroke);
        }
    }
}

/// The ring's button: the ring and the share, hidden until the session said its window.
fn ring(ui: Ui) -> impl IntoView {
    let t = ui.colors;
    let st = ui.state::<Context>();
    let drawn = Ring { id: ViewId::new(), ui };
    let id = drawn.id;
    // Painted again when what it shows changes, or the theme.
    let shown = create_memo(move |_| st.window.with(|w| (w.percent(), w.level(), w.size)));
    create_effect(move |_| {
        shown.get();
        t.with(|_| ());
        id.request_paint();
    });
    let button = h_stack((
        drawn.style(|s| s.width(16.0).height(16.0)),
        label(move || format!("{}%", shown.get().0)).style(|s| s.font_size(12.0).selectable(false)),
    ));
    let me = button.id();
    st.ring.set(Some(me));
    let tip = move || {
        let w = st.window.get();
        format!("Context: {}% full. Details and compact", w.percent())
    };
    floem::views::tooltip(button, move || label(tip))
        .on_click_stop(move |_| {
            let open = !st.open.get_untracked();
            st.open.set(open);
        })
        .style(move |s| {
            let c = t.get();
            s.height(24.0)
                .padding_horiz(4.0)
                .gap(4.0)
                .items_center()
                .border_radius(4.0)
                .color(c.desc)
                .cursor(CursorStyle::Pointer)
                .hover(|s| s.background(c.toolbar_hover))
                .apply_if(shown.get().2 == 0, |s| s.hide())
        })
}

/// The width of the popover, and of its bar within.
const WIDE: f64 = 320.0;
const BAR: f64 = WIDE - 24.0;

/// A small grey line of words.
fn note(t: Theme, text: String) -> impl IntoView {
    label(move || text.clone()).style(move |s| {
        s.font_size(12.0).line_height(1.4).color(t.get().desc).text_overflow(floem::style::TextOverflow::Wrap)
    })
}

/// A row of the list: its swatch, its name, its tokens and its share of the window.
fn kind_row(t: Theme, color: Color, name: String, tokens: u64, size: u64) -> impl IntoView {
    let share = tokens * 100 / size.max(1);
    h_stack((
        empty().style(move |s| s.width(10.0).height(10.0).border_radius(2.0).background(color).flex_shrink(0.0_f32)),
        label(move || name.clone()).style(|s| s.flex_grow(1.0_f32).min_width(0.0).text_ellipsis()),
        label(move || short(tokens)).style(move |s| s.color(t.get().desc)),
        label(move || format!("{share}%")).style(move |s| s.width(36.0).justify_end().color(t.get().desc)),
    ))
    .style(|s| s.items_center().gap(8.0).font_size(12.0).width_full())
}

/// The bar of the whole window: each kind its share, the room for the answer at the right, and the threshold.
pub fn bar(t: Theme, w: &Window, d: Option<&Detail>, width: f64) -> AnyView {
    let c = t.get_untracked();
    let px = |n: u64| (n as f64 / w.size.max(1) as f64 * width).min(width);
    let mut parts: Vec<AnyView> = match d {
        Some(d) => d
            .parts
            .iter()
            .filter(|p| p.tokens > 0)
            .map(|p| {
                let (color, wide) = (kind_color(&c, &p.kind), px(p.tokens));
                empty().style(move |s| s.width(wide).height_full().background(color)).into_any()
            })
            .collect(),
        None => {
            let wide = px(w.used);
            vec![empty().style(move |s| s.width(wide).height_full().background(c.progress)).into_any()]
        }
    };
    parts.push(empty().style(|s| s.flex_grow(1.0_f32)).into_any());
    let reserve = px(w.reserve);
    parts.push(empty().style(move |s| s.width(reserve).height_full().background(c.charts_lines)).into_any());
    let tick = (w.at > 0).then(|| {
        let left = (width * w.at as f64 / 100.0 - 1.0).max(0.0);
        empty()
            .style(move |s| s.absolute().inset_left(left).inset_top(0.0).width(2.0).height_full().background(c.charts_fg))
            .into_any()
    });
    let row = h_stack_from_iter(parts).style(|s| s.width_full().height_full().gap(1.0));
    let mut all = vec![row.into_any()];
    all.extend(tick);
    floem::views::stack_from_iter(all)
        .style(move |s| {
            s.width(width).height(10.0).border_radius(5.0).background(c.input_bg).border(1.0).border_color(c.widget_border)
        })
        .into_any()
}

/// The list under the bar: each kind, the room kept for the answer, what is free.
pub fn kinds(t: Theme, w: &Window, d: &Detail) -> AnyView {
    let c = t.get_untracked();
    let mut rows: Vec<AnyView> = d
        .parts
        .iter()
        .filter(|p| p.tokens > 0)
        .map(|p| kind_row(t, kind_color(&c, &p.kind), p.kind.clone(), p.tokens, w.size).into_any())
        .collect();
    if w.reserve > 0 {
        rows.push(kind_row(t, c.charts_lines, "Kept for the answer".into(), w.reserve, w.size).into_any());
    }
    let free = w.size.saturating_sub(w.used + w.reserve);
    rows.push(kind_row(t, c.input_bg, "Free".into(), free, w.size).into_any());
    v_stack_from_iter(rows).style(|s| s.gap(4.0).width_full()).into_any()
}

/// The popover's content, for what the window and the detail are now.
fn body(ui: Ui, send: Send, w: Window, d: Option<Detail>) -> AnyView {
    let t = ui.colors;
    let st = ui.state::<Context>();
    let mut rows: Vec<AnyView> = vec![
        label(|| "Context").style(|s| s.font_size(13.0).font_weight(Weight::SEMIBOLD)).into_any(),
        note(t, use_line(&w)).into_any(),
    ];
    if let Some(over) = over_line(&w) {
        rows.push(label(move || over.clone()).style(move |s| s.font_size(12.0).color(t.get().warning)).into_any());
    }
    rows.push(bar(t, &w, d.as_ref(), BAR));
    match &d {
        Some(d) => {
            rows.push(kinds(t, &w, d));
            let big: Vec<AnyView> = d
                .biggest
                .iter()
                .take(3)
                .map(|b| {
                    let (name, start, tokens) = (role_name(&b.role), b.start.clone(), b.tokens);
                    h_stack((
                        v_stack((
                            label(move || name).style(|s| s.text_ellipsis()),
                            label(move || start.clone()).style(move |s| s.text_ellipsis().color(t.get().desc)),
                        ))
                        .style(|s| s.flex_grow(1.0_f32).min_width(0.0)),
                        label(move || short(tokens)).style(move |s| s.color(t.get().desc)),
                    ))
                    .style(|s| s.items_center().gap(6.0).font_size(12.0).width_full())
                    .into_any()
                })
                .collect();
            if !big.is_empty() {
                rows.push(label(|| "Biggest").style(|s| s.font_size(12.0).font_weight(Weight::SEMIBOLD)).into_any());
                rows.push(v_stack_from_iter(big).style(|s| s.gap(4.0).width_full()).into_any());
            }
        }
        None => rows.push(note(t, "Reading what fills the window…".into()).into_any()),
    }
    rows.push(
        note(
            t,
            format!(
                "{} Its older messages then become a summary the model reads instead. Compact now to carry on lighter (also: send /compact).",
                compacts(&w)
            ),
        )
        .into_any(),
    );
    let (details, compact) = (send.clone(), send);
    let running = ui.running;
    let act = move |text: &'static str, icon: &'static str, on: Box<dyn Fn()>, off: Box<dyn Fn() -> bool>| {
        h_stack((crate::icons::icon(icon, 14.0), label(move || text)))
            .on_click_stop(move |_| {
                if !off() {
                    on();
                }
            })
            .style(move |s| {
                let c = t.get();
                s.flex_grow(1.0_f32)
                    .flex_basis(0.0)
                    .justify_center()
                    .items_center()
                    .gap(6.0)
                    .padding_vert(4.0)
                    .border_radius(4.0)
                    .font_size(12.0)
                    .background(c.button2_bg)
                    .color(c.button2_fg)
                    .cursor(CursorStyle::Pointer)
                    .hover(|s| s.background(c.button2_hover))
            })
            .into_any()
    };
    rows.push(
        h_stack((
            act(
                "What fills it",
                "list-tree",
                Box::new(move || {
                    st.open.set(false);
                    show_view(ui, &details);
                }),
                Box::new(|| false),
            ),
            act(
                "Compact now",
                "fold",
                Box::new(move || {
                    st.open.set(false);
                    compact_now(ui, &compact);
                }),
                Box::new(move || running.get_untracked()),
            )
            .style(move |s| s.apply_if(running.get(), |s| s.color(t.get().desc).cursor(CursorStyle::Default))),
        ))
        .style(|s| s.gap(8.0).width_full())
        .into_any(),
    );
    v_stack_from_iter(rows).style(|s| s.gap(10.0).width_full()).into_any()
}

/// The popover's layer: over the window while open, at the ring's right and over it; a press outside closes it.
fn popover(ui: Ui, send: &Send) -> AnyView {
    let t = ui.colors;
    let st = ui.state::<Context>();
    let send = send.clone();
    // Built again only when what it shows changes (not with every status).
    let what = create_memo(move |_| st.open.get().then(|| (st.window.get(), st.detail.get())));
    let asked = send.clone();
    create_effect(move |was: Option<bool>| {
        let open = st.open.get();
        if open && was != Some(true) {
            ask(&asked);
        }
        open
    });
    let panel = dyn_container(
        move || what.get(),
        move |shown| match shown {
            None => empty().into_any(),
            Some((w, d)) => body(ui, send.clone(), w, d),
        },
    )
    .on_event_stop(floem::event::EventListener::PointerDown, |_| {})
    .style(move |s| {
        let c = t.get();
        // At the ring's right edge, its bottom a little over the ring's top.
        let (right, bottom) = match st.ring.get().filter(|_| st.open.get()) {
            Some(id) => {
                let r = id.layout_rect();
                let window = window_size(ui, Some(id));
                ((window.width - r.x1).max(8.0), (window.height - r.y0 + 6.0).max(8.0))
            }
            None => (8.0, 8.0),
        };
        s.absolute()
            .inset_right(right)
            .inset_bottom(bottom)
            .width(WIDE)
            .padding(12.0)
            .border(1.0)
            .border_color(c.widget_border)
            .border_radius(6.0)
            .background(c.widget)
            .color(c.fg)
            .font_size(13.0)
            .box_shadow_blur(8.0)
            .box_shadow_color(c.shadow)
            .selectable(false)
    });
    container(panel)
        .on_event_stop(floem::event::EventListener::PointerDown, move |_| st.open.set(false))
        .on_event_stop(floem::event::EventListener::KeyDown, move |e| {
            if let floem::event::Event::KeyDown(k) = e
                && k.key.logical_key == floem::keyboard::Key::Named(floem::keyboard::NamedKey::Escape)
            {
                st.open.set(false);
            }
        })
        .style(move |s| s.absolute().inset(0.0).apply_if(!st.open.get(), |s| s.hide()))
        .into_any()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(used: u64, size: u64, at: u64) -> Window {
        Window { used, size, at, reserve: 4_096, source: "default".into(), ..Window::default() }
    }

    #[test]
    fn tokens_are_written_in_thousands_with_two_decimals_at_most() {
        assert_eq!(short(512), "512");
        assert_eq!(short(8_600), "8.6K");
        assert_eq!(short(32_768), "32.77K");
        assert_eq!(short(4_000), "4K");
        assert_eq!(short(1_048_576), "1.05M");
    }

    #[test]
    fn the_line_says_about_while_estimated_and_the_share_used() {
        let mut w = window(8_600, 32_768, 80);
        assert_eq!(use_line(&w), "About 8.6K of 32.77K tokens (26%); the next answer counts it");
        w.counted = true;
        assert_eq!(use_line(&w), "8.6K of 32.77K tokens (26%)");
        assert_eq!(over_line(&w), None);
        w.used = 30_000;
        assert_eq!(
            over_line(&w).as_deref(),
            Some("With the 4.1K kept for the answer, this is more than the context: the next message compacts the chat first.")
        );
    }

    #[test]
    fn the_ring_warns_near_the_threshold_and_is_full_at_it() {
        assert_eq!(window(10_000, 100_000, 80).level(), Level::Fine);
        assert_eq!(window(65_000, 100_000, 80).level(), Level::Near);
        assert_eq!(window(80_000, 100_000, 80).level(), Level::Full);
        // Only when needed: it warns towards 90%.
        assert_eq!(window(80_000, 100_000, 0).level(), Level::Near);
        // What it holds and the answer's room are more than the window.
        assert_eq!(window(30_000, 32_768, 95).level(), Level::Full);
    }

    #[test]
    fn when_it_compacts_says_who_set_it() {
        let mut w = window(0, 32_768, 80);
        assert_eq!(compacts(&w), "Compacts at 80% (the default), and always before a message would not fit.");
        w.at = 0;
        w.source = "settings".into();
        assert_eq!(compacts(&w), "Compacts only when a message would not fit (set in settings.json).");
    }

    #[test]
    fn the_window_takes_what_status_steps_and_the_detail_say() {
        let w = Window::default().with(&json!({"type": "status", "context": {"used": 900, "size": 32768, "compact_at": 80},
            "compact": {"at": 80, "target": 25, "source": "account", "account": 80, "reserve": 4096}}));
        assert_eq!((w.used, w.size, w.at, w.target, w.reserve, w.source.as_str()), (900, 32768, 80, 25, 4096, "account"));
        assert!(!w.counted);
        let w = w.with(&json!({"type": "step_end", "context": {"used": 1200, "size": 32768, "compact_at": 80}}));
        assert_eq!((w.used, w.counted, w.target), (1200, true, 25));
        assert_eq!(w.with(&json!({"type": "step_end"})), w, "a step that said nothing changes nothing");
        let d = Detail::of(&json!({"kinds": [{"kind": "Tools", "tokens": 300, "pieces": [{"name": "read_file", "tokens": 120},
            {"name": "3 more", "tokens": 10, "more": true}]}], "reserve": 4096, "free": 100, "counted": "estimate",
            "biggest": [{"role": "you", "tokens": 50, "start": "hello"}], "could_free": [{"group": "compact", "tokens": 700}]}));
        assert_eq!(d.parts[0].pieces[1], Piece { name: "3 more".into(), tokens: 10, more: true });
        assert_eq!((d.could_free[0].0.as_str(), d.could_free[0].1, role_name(&d.biggest[0].role)), ("compact", 700, "Your message"));
    }
}
