//! The layout's commands, as VS Code's View > Appearance: the panel moved to the editor's left, right or foot, Zen
//! mode, full screen, the keys into the primary side bar; and the window's title, which follows the folder. What
//! every part has (`register`, `take`) is said in `ext.rs`.
//!
//! Zen mode (Ctrl+K Z) hides every group of the dock, the activity bar and the status bar, and puts the window in
//! full screen; leaving it gives back exactly what was shown before (the groups hidden then, and full screen only if
//! it was on). What is changed while in Zen (Ctrl+B shows the side bar) is not kept past it, as in VS Code.
//!
//! Full screen (F11): Floem 0.2 has no full screen once the window is open, so the window is made to cover its
//! monitor (its bounds before are kept) and the title bar is hidden; F11 again puts the bounds back. Where a client
//! may not place its window (Wayland), the window is not where it was asked to be a moment after, and is then
//! maximized instead (and given back as it was).
//!
//! The Konami code (up up down down left right left right b a), typed where no box has the keys, throws confetti.

use std::cell::Cell;
use std::collections::HashSet;

use floem::keyboard::{Key, Modifiers, NamedKey};
use floem::kurbo::Rect;
use floem::peniko::Color;
use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::{Decorators, empty, stack_from_iter};
use floem::{IntoView, View as _, ViewId, WindowIdExt};
use serde_json::Value;

use crate::dock::{Dir, Dock, EDITOR, HOMES, Home, Node, Side};
use crate::layout::View;
use crate::state::{Send, Ui};

pub const ZEN: &str = "workbench.action.toggleZenMode";
pub const EXIT_ZEN: &str = "workbench.action.exitZenMode";
pub const FULL_SCREEN: &str = "workbench.action.toggleFullScreen";
pub const PANEL_BOTTOM: &str = "workbench.action.positionPanelBottom";
pub const PANEL_LEFT: &str = "workbench.action.positionPanelLeft";
pub const PANEL_RIGHT: &str = "workbench.action.positionPanelRight";
pub const FOCUS_SIDE_BAR: &str = "workbench.action.focusSideBar";

/// The keys of the Konami code, in order.
const KONAMI: [&str; 10] = ["up", "up", "down", "down", "left", "right", "left", "right", "b", "a"];
/// What the toast says when the code is typed.
pub const CHEAT: &str = "Cheat code accepted \u{2014} git push --force is still not a good idea.";
/// The confetti's pieces, and how many frames (of 33 ms) it falls for.
const PIECES: usize = 70;
const FRAMES: u32 = 90;

thread_local! {
    /// A view of the main window (the confetti's layer), by which its window is found.
    static MAIN: Cell<Option<ViewId>> = const { Cell::new(None) };
    /// Where each title bar menu's title begins across the window, as laid out.
    static TITLES: Cell<[Option<f64>; 8]> = const { Cell::new([None; 8]) };
    /// How far into the Konami code the keys typed are.
    static TYPED: Cell<usize> = const { Cell::new(0) };
}

/// What was shown before Zen mode, given back when it ends.
#[derive(Clone, Debug, PartialEq)]
pub struct Zen {
    pub hidden: HashSet<u32>,
    /// Whether the window was in full screen already.
    pub full: bool,
}

/// The window as it was before full screen: its bounds on the screen, and whether it was maximized.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Full {
    pub bounds: Option<Rect>,
    pub maximized: bool,
}

#[derive(Clone, Copy)]
struct Layout {
    zen: RwSignal<Option<Zen>>,
    full: RwSignal<Option<Full>>,
    /// The confetti's frame; none while none falls.
    confetti: RwSignal<Option<u32>>,
}

impl crate::ext::State for Layout {
    fn new(ui: Ui) -> Layout {
        Layout {
            zen: ui.scope.create_rw_signal(None),
            full: ui.scope.create_rw_signal(None),
            confetti: ui.scope.create_rw_signal(None),
        }
    }
}

/// Whether the title bar is drawn: not in full screen.
pub fn title_bar_shown(ui: Ui) -> bool {
    ui.state::<Layout>().full.with(Option::is_none)
}

/// Whether the activity bar and the status bar are drawn: not in Zen mode.
pub fn bars_shown(ui: Ui) -> bool {
    ui.state::<Layout>().zen.with(Option::is_none)
}

pub fn in_zen(ui: Ui) -> bool {
    !bars_shown(ui)
}

pub fn in_full_screen(ui: Ui) -> bool {
    !title_bar_shown(ui)
}

/// Where the panel is against the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Bottom,
    Left,
    Right,
    /// Somewhere else (above the editor, or docked away from it).
    Elsewhere,
}

/// The panel: the group holding the first of the panel's own views (the Terminal's) that is in the tree.
pub fn panel_group(d: &Dock) -> Option<u32> {
    HOMES.iter().filter(|(h, _)| *h == Home::Panel).flat_map(|(_, views)| views.iter()).find_map(|v| d.group_of(*v))
}

/// The split holding the part `id`, and its place in it.
fn parent_of(n: &Node, id: u32) -> Option<(&Node, usize)> {
    let Node::Split { kids, .. } = n else { return None };
    if let Some(at) = kids.iter().position(|k| k.id() == id) {
        return Some((n, at));
    }
    kids.iter().find_map(|k| parent_of(k, id))
}

/// Where the panel is: beside the editor, or under it, in the split they share.
pub fn panel_place(d: &Dock) -> Place {
    let Some(g) = panel_group(d) else { return Place::Elsewhere };
    let Some((Node::Split { dir, kids, .. }, at)) = parent_of(&d.root, g) else { return Place::Elsewhere };
    let Some(editor) = kids.iter().position(Node::has_editor) else { return Place::Elsewhere };
    match (dir, at > editor) {
        (Dir::Column, true) => Place::Bottom,
        (Dir::Row, false) => Place::Left,
        (Dir::Row, true) => Place::Right,
        _ => Place::Elsewhere,
    }
}

/// The panel moved against the editor, its views and the one it shows as they were (as a group of its own, under
/// a new number); none when there is no panel.
pub fn panel_moved(d: &Dock, place: Place) -> Option<(Dock, u32)> {
    let side = match place {
        Place::Bottom => Side::Bottom,
        Place::Left => Side::Left,
        Place::Right => Side::Right,
        Place::Elsewhere => return None,
    };
    let g = panel_group(d)?;
    let Some(Node::Tabs { views, active, .. }) = d.find(g) else { return None };
    let first = *views.first()?;
    let mut next = d.docked(first, EDITOR, side);
    let group = next.group_of(first)?;
    for v in &views[1..] {
        next = next.docked(*v, group, Side::Center);
    }
    Some((next.shown(*active), group))
}

/// The groups hidden in Zen mode: all of them.
pub fn zen_hidden(d: &Dock) -> HashSet<u32> {
    let mut groups = Vec::new();
    d.root.groups(&mut groups);
    groups.into_iter().collect()
}

/// How far into the Konami code a key takes the keys typed before it (at `at`); the code is whole at 10.
pub fn konami_step(at: usize, key: &str) -> usize {
    let at = if at >= KONAMI.len() { 0 } else { at };
    if KONAMI[at] == key {
        at + 1
    } else if key == "up" {
        // "up up up": the last two are still the code's start.
        if at == 2 { 2 } else { 1 }
    } else {
        0
    }
}

fn set_hidden(ui: Ui, next: HashSet<u32>) {
    if ui.hidden.with_untracked(|h| *h != next) {
        ui.hidden.set(next);
    }
}

fn window() -> Option<floem::window::WindowId> {
    MAIN.with(Cell::get).and_then(|v| v.window_id())
}

/// Two bounds the same to a pixel.
fn same(a: Rect, b: Rect) -> bool {
    (a.x0 - b.x0).abs() < 1.0 && (a.y0 - b.y0).abs() < 1.0 && (a.x1 - b.x1).abs() < 1.0 && (a.y1 - b.y1).abs() < 1.0
}

pub fn toggle_full_screen(ui: Ui) {
    let full = ui.state::<Layout>().full;
    match full.get_untracked() {
        Some(was) => {
            full.set(None);
            if let Some(w) = window() {
                if !was.maximized {
                    w.maximized(false);
                }
                if let Some(b) = was.bounds {
                    w.set_window_outer_bounds(b);
                }
                if was.maximized {
                    w.maximized(true);
                }
            }
        }
        None => {
            let w = window();
            let was = Full {
                bounds: w.and_then(|w| w.bounds_on_screen_including_frame()),
                maximized: w.is_some_and(|w| w.is_maximized()),
            };
            full.set(Some(was));
            let Some(w) = w else { return };
            let Some(m) = w.monitor_bounds() else {
                w.maximized(true);
                return;
            };
            if was.maximized {
                w.maximized(false);
            }
            w.set_window_outer_bounds(m);
            // Where the window may not be placed (Wayland), it is maximized instead.
            floem::action::exec_after(std::time::Duration::from_millis(200), move |_| {
                if full.get_untracked().is_some() && !w.bounds_on_screen_including_frame().is_some_and(|b| same(b, m)) {
                    w.maximized(true);
                }
            });
        }
    }
}

pub fn toggle_zen(ui: Ui) {
    let st = ui.state::<Layout>();
    match st.zen.get_untracked() {
        Some(was) => {
            st.zen.set(None);
            set_hidden(ui, was.hidden);
            if !was.full && in_full_screen(ui) {
                toggle_full_screen(ui);
            }
        }
        None => {
            let was = Zen { hidden: ui.hidden.get_untracked(), full: in_full_screen(ui) };
            st.zen.set(Some(was));
            set_hidden(ui, ui.dock.with_untracked(zen_hidden));
            if !in_full_screen(ui) {
                toggle_full_screen(ui);
            }
        }
    }
}

fn move_panel(ui: Ui, place: Place) {
    let Some((next, group)) = ui.dock.with_untracked(|d| panel_moved(d, place)) else { return };
    let old = ui.dock.with_untracked(panel_group);
    ui.dock.set(next);
    // The panel is shown where it went, at VS Code's size for that side.
    let mut hidden = ui.hidden.get_untracked();
    hidden.retain(|g| Some(*g) != old && *g != group);
    set_hidden(ui, hidden);
    let size = ui.size(group);
    let want = if place == Place::Bottom { 230.0 } else { 300.0 };
    if size.get_untracked() != want {
        size.set(want);
    }
}

/// Ctrl+0: the primary side bar shown, with the view it shows.
fn focus_side_bar(ui: Ui) {
    let shown = ui.drawn().with_untracked(|d| match d.side_bar().and_then(|g| d.find(g)) {
        Some(Node::Tabs { active, .. }) => Some(*active),
        _ => None,
    });
    if let Some(v) = shown.or(Some(View::Files)) {
        ui.reveal(v);
    }
}

/// A key that came to the window with no box to take it: one more of the Konami code, or the code begun again.
pub fn key_seen(ui: Ui, key: &Key, m: Modifiers) {
    if m.intersects(Modifiers::CONTROL | Modifiers::ALT | Modifiers::META) {
        return;
    }
    let name = match key {
        Key::Named(NamedKey::ArrowUp) => "up",
        Key::Named(NamedKey::ArrowDown) => "down",
        Key::Named(NamedKey::ArrowLeft) => "left",
        Key::Named(NamedKey::ArrowRight) => "right",
        Key::Character(c) if c.eq_ignore_ascii_case("b") => "b",
        Key::Character(c) if c.eq_ignore_ascii_case("a") => "a",
        Key::Named(NamedKey::Shift) => return,
        _ => "",
    };
    let at = konami_step(TYPED.with(Cell::get), name);
    TYPED.with(|t| t.set(at));
    if at == KONAMI.len() {
        TYPED.with(|t| t.set(0));
        crate::widgets::toast::info(ui, CHEAT);
        throw_confetti(ui);
    }
}

fn throw_confetti(ui: Ui) {
    let confetti = ui.state::<Layout>().confetti;
    if confetti.get_untracked().is_some() {
        return;
    }
    confetti.set(Some(0));
    fn tick(confetti: RwSignal<Option<u32>>) {
        floem::action::exec_after(std::time::Duration::from_millis(33), move |_| {
            let next = confetti.get_untracked().map(|f| f + 1).filter(|f| *f < FRAMES);
            confetti.set(next);
            if next.is_some() {
                tick(confetti);
            }
        });
    }
    tick(confetti);
}

/// A piece of confetti: where it starts across (0..1), how fast it falls and drifts, its colour.
fn piece(n: usize) -> (f64, f64, f64, Color) {
    // A small fixed scatter, the same each time.
    let h = (n as u64).wrapping_mul(2_654_435_761) % 1000;
    let across = h as f64 / 1000.0;
    let speed = 6.0 + (h % 7) as f64;
    let drift = (h % 5) as f64 - 2.0;
    let colours = [
        Color::rgb8(0x00, 0x7A, 0xCC),
        Color::rgb8(0xF1, 0x4C, 0x4C),
        Color::rgb8(0x23, 0xD1, 0x8B),
        Color::rgb8(0xF5, 0xF5, 0x43),
        Color::rgb8(0xBC, 0x3F, 0xBC),
    ];
    (across, speed, drift, colours[n % colours.len()])
}

/// The confetti's layer: over the window while it falls, hidden after. Its view is how the main window is found.
fn confetti_layer(ui: Ui) -> floem::AnyView {
    let confetti = ui.state::<Layout>().confetti;
    let pieces = (0..PIECES).map(move |n| {
        let (across, speed, drift, colour) = piece(n);
        empty().style(move |s| {
            let f = confetti.get().unwrap_or(0) as f64;
            let (w, _) = ui.window.get_untracked();
            s.absolute()
                .width(8.0)
                .height(4.0 + (n % 3) as f64 * 2.0)
                .border_radius(1.0)
                .background(colour)
                .inset_left(across * w + drift * f)
                .inset_top(-20.0 - (n % 9) as f64 * 30.0 + speed * f)
        })
    });
    let layer = stack_from_iter(pieces).style(move |s| {
        s.absolute().inset(0.0).size_full().z_index(110).apply_if(confetti.with(Option::is_none), |s| s.hide())
    });
    MAIN.with(|m| m.set(Some(layer.id())));
    layer.into_any()
}

/// What it adds to the window's registries, once at start.
pub fn register(ui: Ui, _send: &Send) {
    use crate::commands::{add, command};
    use crate::menus::{GO, HELP, Menu, TERMINAL, VIEW, item};
    add(command(ZEN, "View", "Toggle Zen Mode", |ui, _| toggle_zen(ui)).keys("Ctrl+K Z"));
    add(command(EXIT_ZEN, "View", "Exit Zen Mode", |ui, _| toggle_zen(ui)).when(in_zen));
    add(command(FULL_SCREEN, "View", "Toggle Full Screen", |ui, _| toggle_full_screen(ui)).keys("F11"));
    let panel = |id: &str, title: &str, place: Place| {
        command(id, "View", title, move |ui, _| move_panel(ui, place))
            .enabled(move |ui| ui.dock.with(|d| panel_group(d).is_some() && panel_place(d) != place))
    };
    add(panel(PANEL_LEFT, "Move Panel Left", Place::Left));
    add(panel(PANEL_RIGHT, "Move Panel Right", Place::Right));
    add(panel(PANEL_BOTTOM, "Move Panel To Bottom", Place::Bottom));
    add(command(FOCUS_SIDE_BAR, "View", "Focus into Primary Side Bar", |ui, _| focus_side_bar(ui)).keys("Ctrl+0"));

    for (n, (letter, name)) in MNEMONICS.into_iter().enumerate() {
        let open = move |ui: Ui, _: &Send| {
            if ui.menu.get_untracked() != Some(n) {
                ui.menu.set(Some(n));
            }
        };
        add(command(&format!("codeArena.menu.{name}"), "", name, open).keys(&format!("Alt+{letter}")).hidden());
    }
    add(command("workbench.action.menubarFocus", "", "Focus Application Menu", |ui, _| ui.menu.set(Some(0)))
        .keys("F10")
        .hidden());

    // VS Code's View > Appearance, in the View menu's parts group; the others' first rows made live.
    let bar = Menu::TitleBar;
    crate::menus::add(bar(VIEW), item(crate::workbench::SHOW_COMMANDS).title("Command Palette...").group("0_commands"));
    crate::menus::add(bar(VIEW), item(FULL_SCREEN).title("Full Screen").group("2_parts").order(-2));
    crate::menus::add(bar(VIEW), item(ZEN).title("Zen Mode").group("2_parts").order(-1));
    crate::menus::add(bar(VIEW), item(PANEL_LEFT).group("2_parts").order(4));
    crate::menus::add(bar(VIEW), item(PANEL_RIGHT).group("2_parts").order(5));
    crate::menus::add(bar(VIEW), item(PANEL_BOTTOM).group("2_parts").order(6));
    crate::menus::add(bar(GO), item(crate::workbench::QUICK_OPEN).title("Go to File...").group("2_files"));
    crate::menus::add(bar(TERMINAL), item(crate::workbench::NEW_TERMINAL).title("New Terminal").group("1_new"));
    crate::menus::add(bar(TERMINAL), item(crate::workbench::KILL_TERMINAL).title("Kill Terminal").group("2_kill"));
    crate::menus::add(bar(HELP), item(crate::workbench::SHOW_COMMANDS).title("Show All Commands").group("1_welcome"));

    crate::layers::add(crate::layers::Place::Over, "confetti", 70, |ui, _| confetti_layer(ui));

    // The window's title: the folder's name and the app's, as VS Code's "${rootName} · Code Arena".
    let folder = ui.folder;
    ui.scope.create_effect(move |last: Option<String>| {
        let title = window_title(&folder.get());
        if last.as_ref() != Some(&title) {
            floem::action::set_window_title(title.clone());
        }
        title
    });
}

/// A title bar menu's title laid out at `x` across the window.
pub fn title_at(n: usize, x: f64) {
    TITLES.with(|t| {
        let mut all = t.get();
        if let Some(at) = all.get_mut(n) {
            *at = Some(x);
        }
        t.set(all);
    });
}

/// Where a title bar menu's title begins, once it is laid out.
pub fn title_x(n: usize) -> Option<f64> {
    TITLES.with(|t| t.get().get(n).copied().flatten())
}

/// What a key does in a title bar menu that has the keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuKey {
    /// The row lit now (none: none is).
    Lit(Option<usize>),
    /// The menu before (-1) or after (1) it opened in its place.
    Switch(isize),
    /// The row to run.
    Run(usize),
    Close,
    Nothing,
}

/// A key in a title bar menu whose rows that can run are `live` (a line or a dimmed row cannot), the row `lit`
/// lit: VS Code's arrows (around the ends), Home and End, Left and Right to the menus beside it, Enter or Space to
/// run, Escape to close.
pub fn menu_key(live: &[bool], lit: Option<usize>, key: &Key) -> MenuKey {
    let stops: Vec<usize> = (0..live.len()).filter(|i| live[*i]).collect();
    match key {
        Key::Named(NamedKey::ArrowDown) => {
            MenuKey::Lit(stops.iter().copied().find(|i| lit.is_none_or(|l| *i > l)).or(stops.first().copied()))
        }
        Key::Named(NamedKey::ArrowUp) => {
            MenuKey::Lit(stops.iter().rev().copied().find(|i| lit.is_none_or(|l| *i < l)).or(stops.last().copied()))
        }
        Key::Named(NamedKey::Home) => MenuKey::Lit(stops.first().copied()),
        Key::Named(NamedKey::End) => MenuKey::Lit(stops.last().copied()),
        Key::Named(NamedKey::ArrowLeft) => MenuKey::Switch(-1),
        Key::Named(NamedKey::ArrowRight) => MenuKey::Switch(1),
        Key::Named(NamedKey::Enter | NamedKey::Space) => {
            lit.filter(|l| live.get(*l) == Some(&true)).map_or(MenuKey::Nothing, MenuKey::Run)
        }
        Key::Named(NamedKey::Escape | NamedKey::Tab) => MenuKey::Close,
        _ => MenuKey::Nothing,
    }
}

/// The title bar's menus by their letters (VS Code's mnemonics: Alt+F File … Alt+H Help), opened with the keys.
const MNEMONICS: [(&str, &str); 8] = [
    ("F", "file"),
    ("E", "edit"),
    ("S", "selection"),
    ("V", "view"),
    ("G", "go"),
    ("R", "run"),
    ("T", "terminal"),
    ("H", "help"),
];

/// The window's title for a folder.
pub fn window_title(folder: &str) -> String {
    match folder.trim_end_matches('/').rsplit('/').next().filter(|n| !n.is_empty()) {
        Some(name) => format!("{name} \u{b7} Code Arena"),
        None => "Code Arena".into(),
    }
}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The side bar, the editor over a panel of two views, the Agent (Output is not among the views tests begin
    /// with: it is said to be there).
    fn default_dock() -> Dock {
        crate::layout::also_ready(&[View::Output]);
        Dock::of(&[View::Files, View::Terminal, View::Output, View::Agent])
    }

    #[test]
    fn the_panel_is_found_under_the_editor_and_moved_beside_it_and_back() {
        let d = default_dock();
        assert_eq!(panel_place(&d), Place::Bottom);
        let (right, g) = panel_moved(&d, Place::Right).expect("a panel");
        assert_eq!(panel_place(&right), Place::Right);
        assert_eq!(right.group_of(View::Output), Some(g));
        // Its views kept in their order, the editor between the side bar and it, the Agent at the far right.
        match right.find(g) {
            Some(Node::Tabs { views, active, .. }) => {
                assert_eq!(views, &vec![View::Terminal, View::Output]);
                assert_eq!(*active, View::Terminal);
            }
            other => panic!("{other:?}"),
        }
        match &right.root {
            Node::Split { dir: Dir::Row, kids, .. } => {
                assert_eq!(kids.len(), 4);
                assert!(matches!(kids[1], Node::Editor));
                assert_eq!(kids[2].id(), g);
            }
            other => panic!("{other:?}"),
        }
        let (left, _) = panel_moved(&right, Place::Left).expect("a panel");
        assert_eq!(panel_place(&left), Place::Left);
        assert_eq!(left.side_bar(), d.side_bar(), "the side bar stays the side bar");
        let (back, _) = panel_moved(&left, Place::Bottom).expect("a panel");
        assert_eq!(panel_place(&back), Place::Bottom);
        crate::layout::also_ready(&[]);
    }

    #[test]
    fn the_panel_keeps_the_view_it_shows_when_moved() {
        let d = default_dock().shown(View::Output);
        let (moved, g) = panel_moved(&d, Place::Left).expect("a panel");
        assert!(matches!(moved.find(g), Some(Node::Tabs { active: View::Output, .. })));
        assert!(panel_moved(&Dock::of(&[View::Files]), Place::Left).is_none());
        crate::layout::also_ready(&[]);
    }

    #[test]
    fn the_konami_code_is_whole_only_in_its_order() {
        let typed = |keys: &[&str]| keys.iter().fold(0, |at, k| konami_step(at, k));
        assert_eq!(typed(&KONAMI), 10);
        let mut extra = vec!["up"];
        extra.extend(KONAMI);
        assert_eq!(typed(&extra), 10, "up up up down … is the code from its second up");
        assert_eq!(typed(&["up", "up", "down", "x", "down"]), 0);
        assert_eq!(typed(&["up", "down", "up", "up", "down"]), 3);
    }

    #[test]
    fn a_title_bar_menus_keys_go_over_the_rows_that_can_run_and_around_its_ends() {
        let live = [true, false, true, true];
        let key = |k: NamedKey| Key::Named(k);
        assert_eq!(menu_key(&live, None, &key(NamedKey::ArrowDown)), MenuKey::Lit(Some(0)));
        assert_eq!(menu_key(&live, Some(0), &key(NamedKey::ArrowDown)), MenuKey::Lit(Some(2)));
        assert_eq!(menu_key(&live, Some(3), &key(NamedKey::ArrowDown)), MenuKey::Lit(Some(0)));
        assert_eq!(menu_key(&live, None, &key(NamedKey::ArrowUp)), MenuKey::Lit(Some(3)));
        assert_eq!(menu_key(&live, Some(2), &key(NamedKey::ArrowUp)), MenuKey::Lit(Some(0)));
        assert_eq!(menu_key(&live, Some(2), &key(NamedKey::End)), MenuKey::Lit(Some(3)));
        assert_eq!(menu_key(&live, Some(2), &key(NamedKey::Enter)), MenuKey::Run(2));
        assert_eq!(menu_key(&live, Some(1), &key(NamedKey::Enter)), MenuKey::Nothing);
        assert_eq!(menu_key(&live, None, &key(NamedKey::ArrowRight)), MenuKey::Switch(1));
        assert_eq!(menu_key(&live, None, &key(NamedKey::Escape)), MenuKey::Close);
        assert_eq!(menu_key(&[false], None, &key(NamedKey::ArrowDown)), MenuKey::Lit(None));
    }

    #[test]
    fn the_title_is_the_folders_name_and_the_apps() {
        assert_eq!(window_title("/tmp/work"), "work \u{b7} Code Arena");
        assert_eq!(window_title("/tmp/work/"), "work \u{b7} Code Arena");
        assert_eq!(window_title(""), "Code Arena");
    }

    #[test]
    fn zen_mode_hides_every_part_and_gives_back_exactly_what_was_shown() {
        let ui = Ui::new();
        let panel = ui.dock.with_untracked(panel_group).expect("a panel");
        ui.hidden.update(|h| {
            h.insert(panel);
        });
        let before = ui.hidden.get_untracked();
        toggle_zen(ui);
        assert!(in_zen(ui) && in_full_screen(ui));
        assert!(!bars_shown(ui) && !title_bar_shown(ui));
        assert_eq!(ui.hidden.get_untracked(), ui.dock.with_untracked(zen_hidden));
        // Ctrl+B while in Zen shows the side bar; leaving Zen puts things back as they were before it.
        ui.toggle_group_of(View::Files);
        toggle_zen(ui);
        assert_eq!(ui.hidden.get_untracked(), before);
        assert!(bars_shown(ui) && title_bar_shown(ui));
    }

    #[test]
    fn the_panel_commands_may_run_only_where_the_panel_is_not() {
        let ui = Ui::new();
        let send: Send = std::rc::Rc::new(|_| {});
        register(ui, &send);
        let may = |id: &str| crate::commands::find(id).is_some_and(|c| c.may_run(ui));
        assert!(!may(PANEL_BOTTOM) && may(PANEL_RIGHT) && may(PANEL_LEFT));
        assert!(crate::commands::run(ui, &send, PANEL_RIGHT));
        assert_eq!(ui.dock.with_untracked(panel_place), Place::Right);
        assert!(may(PANEL_BOTTOM) && !may(PANEL_RIGHT) && may(PANEL_LEFT));
        let rows = crate::menus::rows(ui, crate::menus::Menu::TitleBar(crate::menus::VIEW));
        let right = rows.iter().find_map(|r| match r {
            crate::menus::Row::Entry { title, command, .. } if title == "Move Panel Right" => Some(command.clone()),
            _ => None,
        });
        assert_eq!(right, Some(None), "dimmed in the View menu");
    }

    #[test]
    fn zen_mode_leaves_full_screen_on_when_it_was_on_before() {
        let ui = Ui::new();
        toggle_full_screen(ui);
        toggle_zen(ui);
        toggle_zen(ui);
        assert!(in_full_screen(ui) && bars_shown(ui));
        toggle_full_screen(ui);
        assert!(title_bar_shown(ui));
    }
}
