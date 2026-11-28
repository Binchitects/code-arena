//! The MCP servers: the status bar's plug with how many are connected of how many (in the warning colours while one
//! is down), and the MCP servers dialog it opens (also MCP: List Servers): each server with a dot for where it
//! stands, what it says (its tools, or why it is not connected), where it is reached, when it is tried again by
//! itself, and Try again; at the foot, which settings turn Arena's tools and Farzan Graph's on and off.
//!
//! The servers come in the driver's `servers` (asked when the session is said, and said again by the driver
//! whenever one changes); Try again sends `servers_retry{name}`, Try All Again `servers_retry{}`.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use floem::event::{Event, EventListener};
use floem::keyboard::{Key, NamedKey};
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith, create_effect, create_memo};
use floem::style::{CursorStyle, TextOverflow};
use floem::views::{
    Decorators, container, dyn_container, empty, h_stack, label, scroll, tooltip, v_stack, v_stack_from_iter,
};
use floem::{AnyView, IntoView, View};
use serde_json::{Value, json};

use crate::commands;
use crate::icons::icon;
use crate::layers::{self, Place};
use crate::state::{Send, Ui};
use crate::statusbar::{self, Side};
use crate::widgets::{MONO_FONT, Theme, button};

/// The command that opens the dialog, as VS Code names it.
pub const LIST: &str = "workbench.mcp.listServer";

/// Where a server stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stands {
    #[default]
    Connecting,
    Connected,
    /// Not served here (Arena has no such endpoint): said quietly.
    Unavailable,
    Failed,
}

/// One server as the session says it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Server {
    pub name: String,
    pub title: String,
    pub url: String,
    pub state: Stands,
    pub tools: u64,
    pub error: Option<String>,
    /// When it is tried again by itself, in ms since 1970.
    pub next_try: Option<u64>,
}

impl Server {
    fn of(v: &Value) -> Option<Server> {
        Some(Server {
            name: v["name"].as_str()?.to_string(),
            title: v["title"].as_str().unwrap_or("").to_string(),
            url: v["url"].as_str().unwrap_or("").to_string(),
            state: match v["state"].as_str().unwrap_or("") {
                "connected" => Stands::Connected,
                "unavailable" => Stands::Unavailable,
                "failed" => Stands::Failed,
                _ => Stands::Connecting,
            },
            tools: v["tools"].as_u64().unwrap_or(0),
            error: v["error"].as_str().map(str::to_string),
            next_try: v["next_try"].as_u64(),
        })
    }

    /// What it says, in a line.
    pub fn status_line(&self) -> String {
        let why = self.error.clone().unwrap_or_default();
        match self.state {
            Stands::Connecting => "Connecting…".into(),
            Stands::Connected if self.tools == 1 => "Connected: 1 tool".into(),
            Stands::Connected => format!("Connected: {} tools", self.tools),
            Stands::Unavailable if why.is_empty() => "Not available here".into(),
            Stands::Unavailable => format!("Not available here: {why}"),
            Stands::Failed if why.is_empty() => "Not connected".into(),
            Stands::Failed => format!("Not connected: {why}"),
        }
    }

    /// When it is tried again by itself, from `now` (ms since 1970).
    pub fn next_line(&self, now: u64) -> Option<String> {
        let at = self.next_try?;
        let secs = at.saturating_sub(now).div_ceil(1000);
        Some(if secs == 0 { "Trying again now".into() } else { format!("Trying again in {secs} s") })
    }

    fn may_retry(&self) -> bool {
        matches!(self.state, Stands::Failed | Stands::Unavailable)
    }
}

/// How many are connected, of how many, and whether one is down.
pub fn tally(all: &[Server]) -> (usize, usize, bool) {
    let on = all.iter().filter(|s| s.state == Stands::Connected).count();
    (on, all.len(), all.iter().any(|s| s.state == Stands::Failed))
}

/// The servers' own state.
#[derive(Clone, Copy)]
pub struct Mcp {
    pub servers: RwSignal<Vec<Server>>,
    pub open: RwSignal<bool>,
    /// The time, ticking each second while the dialog is open (for when a server is tried again).
    now: RwSignal<u64>,
}

impl crate::ext::State for Mcp {
    fn new(ui: Ui) -> Mcp {
        Mcp {
            servers: ui.scope.create_rw_signal(Vec::new()),
            open: ui.scope.create_rw_signal(false),
            now: ui.scope.create_rw_signal(0),
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Opens the MCP servers dialog, and asks where they stand.
pub fn open(ui: Ui, send: &Send) {
    send(json!({"type": "servers"}));
    ui.state::<Mcp>().open.set(true);
}

/// What it adds to the window's registries, once at start: the status bar's plug, the dialog's layer and its
/// command.
pub fn register(_ui: Ui, _send: &Send) {
    statusbar::add("status.mcp", Side::Right, 292, item);
    layers::add(Place::Over, "mcp.servers", 30, layer);
    commands::add(commands::command(LIST, "MCP", "List Servers", open));
}

/// The session's messages that are this part's, taken in. Never taken away from the parts after it.
pub fn take(ui: Ui, m: &Value, send: &Send) -> bool {
    let st = ui.state::<Mcp>();
    match m["type"].as_str().unwrap_or("") {
        "state" => send(json!({"type": "servers"})),
        "servers" => {
            let all: Vec<Server> = m["items"].as_array().into_iter().flatten().filter_map(Server::of).collect();
            if st.servers.with_untracked(|was| *was != all) {
                st.servers.set(all);
            }
        }
        _ => {}
    }
    false
}

/// The status bar's plug: connected/total, hidden while there is no server.
fn item(ui: Ui, send: Send) -> AnyView {
    let t = ui.colors;
    let st = ui.state::<Mcp>();
    let shown = create_memo(move |_| st.servers.with(|all| tally(all)));
    let inner = h_stack((
        icon("plug", 14.0),
        label(move || {
            let (on, all, _) = shown.get();
            format!("{on}/{all}")
        }),
    ))
    .on_click_stop(move |_| open(ui, &send))
    .style(move |s| {
        let c = t.get();
        let (_, all, down) = shown.get();
        statusbar::item(t, s)
            .cursor(CursorStyle::Pointer)
            .apply_if(down, |s| s.background(c.status_warning_bg).color(c.status_warning_fg))
            .apply_if(all == 0, |s| s.hide())
    });
    let tip = move || {
        let (on, all, down) = shown.get();
        let more = if down { "; one is down" } else { "" };
        format!("MCP servers: {on} of {all} connected{more}. Show the servers")
    };
    tooltip(inner, move || label(tip)).style(|s| s.height_full()).into_any()
}

/// The dot of where a server stands.
fn dot(t: Theme, state: Stands) -> impl IntoView {
    empty().style(move |s| {
        let c = t.get();
        let color = match state {
            Stands::Connected => c.ok,
            Stands::Connecting => c.warning,
            Stands::Failed => c.error,
            Stands::Unavailable => c.desc,
        };
        s.width(8.0).height(8.0).border_radius(4.0).background(color).flex_shrink(0.0_f32).margin_top(5.0)
    })
}

/// One server's row.
fn row(ui: Ui, send: Send, s: Server) -> AnyView {
    let t = ui.colors;
    let st = ui.state::<Mcp>();
    let (title, name, status, url) = (s.title.clone(), s.name.clone(), s.status_line(), s.url.clone());
    let failed = s.state == Stands::Failed;
    let next = s.clone();
    let mut lines: Vec<AnyView> = vec![
        h_stack((
            label(move || title.clone()).style(|s| s.font_weight(*crate::widgets::SEMIBOLD).text_ellipsis()),
            label(move || name.clone()).style(move |s| s.color(t.get().desc).font_size(12.0).text_ellipsis()),
        ))
        .style(|s| s.gap(8.0).items_center().min_width(0.0))
        .into_any(),
        label(move || status.clone())
            .style(move |s| {
                let c = t.get();
                s.text_overflow(TextOverflow::Wrap).apply_if(failed, |s| s.color(c.error))
            })
            .into_any(),
    ];
    if !url.is_empty() {
        lines.push(
            label(move || url.clone())
                .style(move |s| {
                    s.font_family(MONO_FONT.clone()).font_size(12.0).color(t.get().desc).text_ellipsis().min_width(0.0)
                })
                .into_any(),
        );
    }
    if s.next_try.is_some() {
        lines.push(
            label(move || next.next_line(st.now.get()).unwrap_or_default())
                .style(move |s| s.font_size(12.0).color(t.get().desc))
                .into_any(),
        );
    }
    let text = v_stack_from_iter(lines).style(|s| s.gap(2.0).flex_grow(1.0_f32).min_width(0.0).line_height(1.4));
    let retry: AnyView = if s.may_retry() {
        let name = s.name.clone();
        button(t, "Try again", false, move || send(json!({"type": "servers_retry", "name": name}))).into_any()
    } else {
        empty().into_any()
    };
    h_stack((dot(t, s.state), text, retry))
        .style(move |s| {
            s.gap(10.0)
                .width_full()
                .items_start()
                .padding_vert(8.0)
                .border_bottom(1.0)
                .border_color(t.get().widget_border)
        })
        .into_any()
}

/// The words at the dialog's foot: what turns the Arena's own servers on and off, and where the person's own are.
pub const NOTE: &str = "Arena's tools and Farzan Graph's are turned on and off by the settings mcp.arenaTools and mcp.codeIntelTools. Your own servers are under mcpServers in settings.json.";

/// The dialog's box, for the servers as they are now.
fn dialog(ui: Ui, send: Send, all: Vec<Server>) -> AnyView {
    let t = ui.colors;
    let st = ui.state::<Mcp>();
    let (on, total, _) = tally(&all);
    let any_retry = all.iter().any(Server::may_retry);
    let rows: Vec<AnyView> = all.into_iter().map(|s| row(ui, send.clone(), s)).collect();
    let empty_note = rows.is_empty().then(|| {
        label(|| "This session has no MCP server.").style(move |s| s.color(t.get().desc).padding_vert(8.0)).into_any()
    });
    let list: AnyView = match empty_note {
        Some(n) => n,
        None => scroll(v_stack_from_iter(rows).style(|s| s.width_full())).style(|s| s.max_height(360.0)).into_any(),
    };
    let again = send.clone();
    let buttons = h_stack((
        empty().style(|s| s.flex_grow(1.0_f32)),
        button(t, "Try All Again", false, move || again(json!({"type": "servers_retry"})))
            .style(move |s| s.apply_if(!any_retry, |s| s.hide())),
        button(t, "Close", true, move || st.open.set(false)),
    ))
    .style(|s| s.gap(8.0).width_full().margin_top(4.0));
    v_stack((
        label(|| "MCP servers").style(|s| s.font_size(15.0).font_weight(*crate::widgets::SEMIBOLD)),
        label(move || format!("{on} of {total} connected"))
            .style(move |s| s.color(t.get().desc).apply_if(total == 0, |s| s.hide())),
        list,
        label(|| NOTE)
            .style(move |s| s.font_size(12.0).line_height(1.4).color(t.get().desc).text_overflow(TextOverflow::Wrap)),
        buttons,
    ))
    .on_event_stop(EventListener::PointerDown, |_| {})
    .style(move |s| {
        let c = t.get();
        s.width(520.0)
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

/// Sets the time now, and again each second while the dialog is open.
fn tick(st: Mcp) {
    if !st.open.get_untracked() {
        return;
    }
    st.now.set(now_ms());
    floem::action::exec_after(Duration::from_secs(1), move |_| tick(st));
}

/// The dialog's layer: the window dimmed under it while open; Escape or a press outside closes it.
fn layer(ui: Ui, send: Send) -> AnyView {
    let st = ui.state::<Mcp>();
    let shown = create_memo(move |_| st.open.get().then(|| st.servers.get()));
    let body = dyn_container(
        move || shown.get(),
        move |all| match all {
            Some(all) => dialog(ui, send.clone(), all),
            None => empty().into_any(),
        },
    );
    let layer = container(body).keyboard_navigable();
    let id = layer.id();
    create_effect(move |was: Option<bool>| {
        let open = st.open.get();
        if open && was != Some(true) {
            id.request_focus();
            tick(st);
        }
        open
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

#[cfg(test)]
mod tests {
    use super::*;

    fn server(v: Value) -> Server {
        Server::of(&v).expect("a server")
    }

    #[test]
    fn a_server_says_where_it_stands() {
        let s = server(json!({"name": "a", "state": "connected", "tools": 12}));
        assert_eq!(s.status_line(), "Connected: 12 tools");
        let s = server(json!({"name": "a", "state": "connected", "tools": 1}));
        assert_eq!(s.status_line(), "Connected: 1 tool");
        let s = server(json!({"name": "a", "state": "failed", "error": "refused"}));
        assert_eq!(s.status_line(), "Not connected: refused");
        assert!(s.may_retry());
        let s = server(json!({"name": "a", "state": "unavailable"}));
        assert_eq!(s.status_line(), "Not available here");
        let s = server(json!({"name": "a"}));
        assert_eq!(s.status_line(), "Connecting…");
        assert!(!s.may_retry());
        assert!(Server::of(&json!({"title": "no name"})).is_none());
    }

    #[test]
    fn the_next_try_counts_down_in_whole_seconds() {
        let s = server(json!({"name": "a", "state": "failed", "next_try": 10_000}));
        assert_eq!(s.next_line(0).as_deref(), Some("Trying again in 10 s"));
        assert_eq!(s.next_line(9_500).as_deref(), Some("Trying again in 1 s"));
        assert_eq!(s.next_line(12_000).as_deref(), Some("Trying again now"));
        assert_eq!(server(json!({"name": "b"})).next_line(0), None);
    }

    #[test]
    fn the_plug_counts_the_connected_and_warns_when_one_failed() {
        let all = vec![
            server(json!({"name": "a", "state": "connected"})),
            server(json!({"name": "b", "state": "unavailable"})),
        ];
        assert_eq!(tally(&all), (1, 2, false));
        let mut all = all;
        all.push(server(json!({"name": "c", "state": "failed"})));
        assert_eq!(tally(&all), (1, 3, true));
        assert_eq!(tally(&[]), (0, 0, false));
    }
}
