//! Zoom and the accessibility settings, as the old Code Arena has them over any colour theme.
//!
//! - `window.zoomLevel` (-5 to 9): the whole window larger or smaller, its text and everything else, each step 20%
//!   as VS Code's (1: 120%, -1: about 83%). **View: Zoom In** (Ctrl+=, Ctrl+Shift+=, Ctrl+Numpad+), **Zoom Out**
//!   (Ctrl+-, Ctrl+Numpad-) and **Reset Zoom** (Ctrl+Numpad0) keep it in the person's settings. The step is taken
//!   from the zoom shown, so two presses before the settings say the first are two steps.
//! - `codeArena.accessibility.textSize` (100%, 112%, 125%, 150%): the workbench larger, the editor's text as
//!   `editor.fontSize` has it (the editor's font is made smaller by as much as the window is made larger).
//! - `codeArena.accessibility.contrast` "more": secondary text as the text, borders that stand out, the focus ring
//!   at full strength ([`adjusted`]).
//! - `codeArena.accessibility.focusRings` "strong": the focus ring in the text's colour, 2 px, on the controls.
//! - `workbench.reduceMotion` "on": spinners stand still ([`still`]).
//! - `codeArena.accessibility.colorBlindColors`: Okabe and Ito's blue and vermilion where red and green say added
//!   and removed, passed and failed (a compare's lines, the gutter's changes, git's letters, the tests).
//!
//! The commands **Toggle High Contrast Theme** (Dark High Contrast, or Light High Contrast for a light theme, and
//! back to the preferred theme), **Toggle More Contrast**, **Toggle Colour-Blind Safe Colours** and **Toggle Reduce
//! Motion** write the settings, as the old Code Arena's do.

use std::cell::Cell;

use floem::keyboard::{Key, KeyLocation};
use floem::peniko::Color;
use floem::reactive::{Memo, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::style::Style;
use serde_json::{Value, json};

use crate::colors::{Colors, hex};
use crate::commands::{add, command};
use crate::settings;
use crate::state::{Send, Ui};

pub const ZOOM_IN: &str = "workbench.action.zoomIn";
pub const ZOOM_OUT: &str = "workbench.action.zoomOut";
pub const ZOOM_RESET: &str = "workbench.action.zoomReset";
pub const TOGGLE_HIGH_CONTRAST: &str = "codeArena.accessibility.toggleHighContrast";
pub const TOGGLE_MORE_CONTRAST: &str = "codeArena.accessibility.toggleMoreContrast";
pub const TOGGLE_COLOR_BLIND: &str = "codeArena.accessibility.toggleColorBlindColors";
pub const TOGGLE_REDUCE_MOTION: &str = "codeArena.accessibility.toggleReduceMotion";

/// The zoom levels VS Code's setting takes here.
const LEAST: f64 = -5.0;
const MOST: f64 = 9.0;

/// The settings followed, and the zoom shown.
#[derive(Clone, Copy)]
pub struct Access {
    zoom: Memo<Value>,
    text: Memo<Value>,
    contrast: Memo<Value>,
    focus: Memo<Value>,
    motion: Memo<Value>,
    colors: Memo<Value>,
    /// The zoom level shown.
    level: RwSignal<f64>,
    /// A level a command asked to keep, until the settings say it (or refuse it).
    pending: RwSignal<Option<f64>>,
}

impl crate::ext::State for Access {
    fn new(ui: Ui) -> Access {
        Access {
            zoom: settings::memo(ui, "window.zoomLevel", || None),
            text: settings::memo(ui, "codeArena.accessibility.textSize", || None),
            contrast: settings::memo(ui, "codeArena.accessibility.contrast", || None),
            focus: settings::memo(ui, "codeArena.accessibility.focusRings", || None),
            motion: settings::memo(ui, "workbench.reduceMotion", || None),
            colors: settings::memo(ui, "codeArena.accessibility.colorBlindColors", || None),
            level: ui.scope.create_rw_signal(0.0),
            pending: ui.scope.create_rw_signal(None),
        }
    }
}

/// How the colours are changed over the theme.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Seeing {
    /// `codeArena.accessibility.contrast` "more".
    pub more: bool,
    /// `codeArena.accessibility.focusRings` "strong".
    pub strong: bool,
    /// `codeArena.accessibility.colorBlindColors`.
    pub safe: bool,
}

/// The colour options as the settings are now. Not followed (the theme is applied again when they change).
pub fn seeing(ui: Ui) -> Seeing {
    let a = ui.state::<Access>();
    Seeing {
        more: a.contrast.get_untracked().as_str() == Some("more"),
        strong: a.focus.get_untracked().as_str() == Some("strong"),
        safe: a.colors.get_untracked().as_bool() == Some(true),
    }
}

/// Whether nothing moves that need not (`workbench.reduceMotion` "on"). Followed.
pub fn still(ui: Ui) -> bool {
    ui.state::<Access>().motion.get().as_str() == Some("on")
}

/// Whether focus rings are strong. Followed.
pub fn strong_rings(ui: Ui) -> bool {
    ui.state::<Access>().focus.get().as_str() == Some("strong")
}

/// How much larger `codeArena.accessibility.textSize` makes the workbench: 1, 1.125, 1.25 or 1.5. Followed.
pub fn text_share(ui: Ui) -> f64 {
    text_factor(&ui.state::<Access>().text.get())
}

/// A text size setting's factor ("112%" is the old Code Arena's 112.5%); 1 for anything else.
pub fn text_factor(v: &Value) -> f64 {
    match v.as_str() {
        Some("112%") => 1.125,
        Some("125%") => 1.25,
        Some("150%") => 1.5,
        _ => 1.0,
    }
}

/// A zoom setting's level: a number from -5 to 9, else 0.
pub fn level_of(v: &Value) -> f64 {
    v.as_f64().filter(|n| n.is_finite()).map_or(0.0, |n| n.clamp(LEAST, MOST))
}

/// The level one step in (`by` 1) or out (-1) from `now`, within -5 and 9.
pub fn stepped(now: f64, by: f64) -> f64 {
    (now + by).clamp(LEAST, MOST)
}

/// The window's scale for a zoom level and a text size: 1.2 to the level, times the text's factor.
pub fn scale(level: f64, text: f64) -> f64 {
    1.2f64.powf(level.clamp(LEAST, MOST)) * text
}

/// A key pressed on the number pad, by VS Code's name for it ("numpad0", "numpad_add"); none for another key.
pub fn numpad(key: &Key, location: KeyLocation) -> Option<Key> {
    let Key::Character(c) = key else { return None };
    if location != KeyLocation::Numpad {
        return None;
    }
    let name = match c.as_str() {
        d @ ("0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9") => format!("numpad{d}"),
        "+" => "numpad_add".into(),
        "-" => "numpad_subtract".into(),
        "*" => "numpad_multiply".into(),
        "/" => "numpad_divide".into(),
        "." | "," => "numpad_decimal".into(),
        _ => return None,
    };
    Some(Key::Character(name.into()))
}

/// Floem's own controls' focus rings with strong rings on (the root's style gives them to every one below).
pub fn rings(ui: Ui, s: Style) -> Style {
    if !strong_rings(ui) {
        return s;
    }
    let c = ui.colors.get();
    let strong = move |s: Style| s.outline_color(c.focus).focus_visible(|s| s.outline(2.0));
    use floem::views::{
        ButtonClass, CheckboxClass, LabeledCheckboxClass, LabeledRadioButtonClass, ListItemClass, RadioButtonClass,
        TextInputClass, ToggleButtonClass,
    };
    s.class(ButtonClass, strong)
        .class(TextInputClass, strong)
        .class(CheckboxClass, strong)
        .class(LabeledCheckboxClass, strong)
        .class(RadioButtonClass, strong)
        .class(LabeledRadioButtonClass, strong)
        .class(ToggleButtonClass, strong)
        .class(ListItemClass, strong)
}

fn opaque(c: Color, ground: Color) -> Color {
    let a = f64::from(c.a) / 255.0;
    let m = |x: u8, y: u8| (f64::from(x) * a + f64::from(y) * (1.0 - a)).round() as u8;
    Color::rgb8(m(c.r, ground.r), m(c.g, ground.g), m(c.b, ground.b))
}

fn mix(a: Color, b: Color, share: f64) -> Color {
    let m = |x: u8, y: u8| (f64::from(x) * share + f64::from(y) * (1.0 - share)).round() as u8;
    Color::rgb8(m(a.r, b.r), m(a.g, b.g), m(a.b, b.b))
}

fn hex_of(c: Color) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
}

/// A colour read as text made 4.5:1 on its ground (laid over it first when it has an alpha of its own).
fn readable(c: Color, ground: Color, dark: bool) -> Color {
    let flat = opaque(c, opaque(ground, if dark { Color::BLACK } else { Color::WHITE }));
    let ground = hex_of(opaque(ground, if dark { Color::BLACK } else { Color::WHITE }));
    hex(&ca_themes::raised(&hex_of(flat), &[&ground], dark)).unwrap_or(c)
}

/// The theme's colours with the accessibility settings over them.
pub fn adjusted(mut c: Colors, s: Seeing) -> Colors {
    let dark = c.dark;
    if s.safe {
        let (blue, vermilion, orange) = (Color::rgb8(0x00, 0x72, 0xB2), Color::rgb8(0xD5, 0x5E, 0x00), Color::rgb8(0xE6, 0x9F, 0x00));
        let (good, bad) = if dark {
            (Color::rgb8(0x56, 0xB4, 0xE9), Color::rgb8(0xF0, 0x8A, 0x4B))
        } else {
            (blue, vermilion)
        };
        c.gutter_added = blue;
        c.gutter_modified = orange;
        c.gutter_deleted = vermilion;
        c.ruler_added = blue.multiply_alpha(0.6);
        c.ruler_modified = orange.multiply_alpha(0.6);
        c.ruler_deleted = vermilion.multiply_alpha(0.6);
        c.diff_added = blue.multiply_alpha(0.25);
        c.diff_removed = vermilion.multiply_alpha(0.25);
        c.diff_added_text = blue.multiply_alpha(0.35);
        c.diff_removed_text = vermilion.multiply_alpha(0.35);
        let (side, editor) = (c.side_bg, c.editor_bg);
        let text = |col: Color| readable(readable(col, side, dark), editor, dark);
        c.added_fg = text(good);
        c.scm_untracked = text(good);
        c.removed_fg = text(bad);
        c.ok = text(good);
    }
    if s.more {
        c.desc = c.fg;
        c.activity_inactive = c.activity_fg;
        c.activity_top_inactive = c.activity_top_fg;
        c.tab_inactive_fg = c.tab_active_fg;
        c.tab_unfocused_active_fg = c.tab_active_fg;
        c.panel_title_inactive = c.panel_title;
        c.breadcrumb = c.fg;
        c.side_title = c.side_fg;
        c.line_number = readable(c.line_number, c.editor_bg, dark);
        c.placeholder = readable(c.placeholder, c.input_bg, dark);
        c.inlay_fg = readable(c.inlay_fg, c.editor_bg, dark);
        let border = |ground: Color| mix(c.fg, opaque(ground, Color::BLACK), 0.55);
        let editor = border(c.editor_bg);
        c.widget_border = editor;
        c.group_border = editor;
        c.panel_border = editor;
        c.tab_border = editor;
        c.tabs_border = editor;
        c.menu_border = editor;
        c.quick_border = editor;
        c.suggest_border = editor;
        c.hover_border = editor;
        c.ruler_border = editor;
        c.key_border = editor;
        c.side_border = border(c.side_bg);
        c.section_border = border(c.side_bg);
        c.activity_border = border(c.activity_bg);
        c.status_border = border(c.status_bg);
        c.title_border = border(c.title_bg);
        c.input_border = mix(c.fg, opaque(c.input_bg, Color::BLACK), 0.65);
        c.list_outline = c.fg;
    }
    if s.more || s.strong {
        c.focus = c.fg;
    }
    c
}

thread_local! {
    /// The scale given to the window last (Floem starts it at 1).
    static SCALE: Cell<f64> = const { Cell::new(1.0) };
}

fn apply_scale(scale: f64) {
    if SCALE.with(|s| (s.get() - scale).abs() > 1e-9) {
        SCALE.with(|s| s.set(scale));
        floem::action::set_window_scale(scale);
    }
}

/// A zoom command: one step in or out, or back to 0 (`by` none). Shown at once, kept in the person's settings.
pub fn zoom(ui: Ui, send: &Send, by: Option<f64>) {
    let a = ui.state::<Access>();
    let now = a.level.get_untracked();
    let next = by.map_or(0.0, |by| stepped(now, by));
    if next == now {
        return;
    }
    a.level.set(next);
    a.pending.set(Some(next));
    let value = if next.fract() == 0.0 { json!(next as i64) } else { json!(next) };
    send(json!({"type": "set_setting", "key": "window.zoomLevel", "value": value}));
}

fn set(send: &Send, key: &str, value: Value) {
    send(json!({"type": "set_setting", "key": key, "value": value}));
}

/// High contrast on (the dark or the light one as the theme is) or off (back to the preferred theme of its kind).
fn toggle_high_contrast(ui: Ui, send: &Send) {
    let now = crate::theme_menu::theme(ui);
    let next = match now.kind() {
        "hc" => settings::text(ui, "workbench.preferredDarkColorTheme", None),
        "hcLight" => settings::text(ui, "workbench.preferredLightColorTheme", None),
        _ if now.is_dark() => "Dark High Contrast".into(),
        _ => "Light High Contrast".into(),
    };
    crate::theme_menu::choose(ui, send, &next);
}

/// What it adds to the window's registries, once at start.
pub fn register(ui: Ui, _send: &Send) {
    add(command(ZOOM_IN, "View", "Zoom In", |ui, send| zoom(ui, send, Some(1.0)))
        .keys("Ctrl+Shift+=")
        .keys("Ctrl+numpad_add"));
    add(command(ZOOM_OUT, "View", "Zoom Out", |ui, send| zoom(ui, send, Some(-1.0))).keys("Ctrl+numpad_subtract"));
    add(command(ZOOM_RESET, "View", "Reset Zoom", |ui, send| zoom(ui, send, None)));
    add(command(TOGGLE_HIGH_CONTRAST, "Preferences", "Toggle High Contrast Theme", toggle_high_contrast));
    add(command(TOGGLE_MORE_CONTRAST, "Preferences", "Toggle More Contrast", |ui, send| {
        let more = settings::text(ui, "codeArena.accessibility.contrast", None) == "more";
        set(send, "codeArena.accessibility.contrast", json!(if more { "standard" } else { "more" }));
    }));
    add(command(TOGGLE_COLOR_BLIND, "Preferences", "Toggle Colour-Blind Safe Colours", |ui, send| {
        let on = settings::flag(ui, "codeArena.accessibility.colorBlindColors", None);
        set(send, "codeArena.accessibility.colorBlindColors", json!(!on));
    }));
    add(command(TOGGLE_REDUCE_MOTION, "Preferences", "Toggle Reduce Motion", |ui, send| {
        let on = settings::text(ui, "workbench.reduceMotion", None) == "on";
        set(send, "workbench.reduceMotion", json!(if on { "auto" } else { "on" }));
    }));
    let a = ui.state::<Access>();
    // The level the settings say, unless a command's is waiting for them: then only that one lets it go.
    ui.scope.create_effect(move |_| {
        let said = level_of(&a.zoom.get());
        match a.pending.get_untracked() {
            Some(p) if p != said => {}
            Some(_) => a.pending.set(None),
            None => {
                if a.level.get_untracked() != said {
                    a.level.set(said);
                }
            }
        }
    });
    ui.scope.create_effect(move |_| apply_scale(scale(a.level.get(), text_factor(&a.text.get()))));
    // The colours again when a colour option changes.
    ui.scope.create_effect(move |_| {
        a.contrast.with(|_| ());
        a.focus.with(|_| ());
        a.colors.with(|_| ());
        ui.apply_theme();
    });
}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    // A zoom the session would not keep: the level the settings say is shown again (theme_menu says why).
    if m["type"] == "setting_refused" && m["key"] == "window.zoomLevel" {
        let a = ui.state::<Access>();
        a.pending.set(None);
        let said = level_of(&a.zoom.get_untracked());
        if a.level.get_untracked() != said {
            a.level.set(said);
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_zoom_step_is_twenty_percent_within_minus_five_and_nine() {
        assert_eq!(scale(0.0, 1.0), 1.0);
        assert!((scale(1.0, 1.0) - 1.2).abs() < 1e-9);
        assert!((scale(-1.0, 1.0) - 1.0 / 1.2).abs() < 1e-9);
        assert!((scale(2.0, 1.25) - 1.8).abs() < 1e-9);
        assert_eq!(stepped(9.0, 1.0), 9.0);
        assert_eq!(stepped(-5.0, -1.0), -5.0);
        assert_eq!(stepped(0.5, 1.0), 1.5);
        assert_eq!(level_of(&json!(12)), 9.0);
        assert_eq!(level_of(&json!("big")), 0.0);
        assert_eq!(text_factor(&json!("112%")), 1.125);
        assert_eq!(text_factor(&json!("90%")), 1.0);
    }

    #[test]
    fn number_pad_keys_have_vs_codes_names() {
        let k = |c: &str| Key::Character(c.into());
        assert_eq!(numpad(&k("0"), KeyLocation::Numpad), Some(k("numpad0")));
        assert_eq!(numpad(&k("+"), KeyLocation::Numpad), Some(k("numpad_add")));
        assert_eq!(numpad(&k("-"), KeyLocation::Numpad), Some(k("numpad_subtract")));
        assert_eq!(numpad(&k("0"), KeyLocation::Standard), None);
        let chord = crate::commands::Chord::of(&k("numpad_add"), floem::keyboard::Modifiers::CONTROL);
        assert_eq!(chord, crate::commands::Chord::parse("Ctrl+numpad_add"));
        let plus = crate::commands::Chord::of(&k("+"), floem::keyboard::Modifiers::CONTROL | floem::keyboard::Modifiers::SHIFT);
        assert_eq!(plus, crate::commands::Chord::parse("Ctrl+Shift+="));
    }

    use std::cell::RefCell;
    use std::rc::Rc;

    fn window() -> (Ui, Send, Rc<RefCell<Vec<Value>>>) {
        let ui = Ui::new();
        let told = Rc::new(RefCell::new(Vec::new()));
        let send: Send = {
            let to = told.clone();
            Rc::new(move |v: Value| to.borrow_mut().push(v))
        };
        ui.out.set(Some(send.clone()));
        crate::settings::register(ui, &send);
        register(ui, &send);
        (ui, send, told)
    }

    fn said(ui: Ui, send: &Send, values: Value) {
        ui.take(&json!({"type": "settings", "values": values, "by_lang": {}, "sources": {}, "problems": [], "files": {}}), send);
    }

    #[test]
    fn two_zoom_steps_before_the_settings_say_the_first_are_two_and_the_file_wins_after() {
        let (ui, send, told) = window();
        let a = ui.state::<Access>();
        zoom(ui, &send, Some(1.0));
        zoom(ui, &send, Some(1.0));
        assert_eq!(a.level.get_untracked(), 2.0);
        let kept: Vec<Value> = told.borrow().iter().filter(|m| m["key"] == "window.zoomLevel").map(|m| m["value"].clone()).collect();
        assert_eq!(kept, vec![json!(1), json!(2)]);
        said(ui, &send, json!({"window.zoomLevel": 1}));
        assert_eq!(a.level.get_untracked(), 2.0, "the first step's echo does not undo the second");
        said(ui, &send, json!({"window.zoomLevel": 2}));
        assert_eq!(a.pending.get_untracked(), None);
        said(ui, &send, json!({"window.zoomLevel": -7}));
        assert_eq!(a.level.get_untracked(), -5.0, "the file edited by hand, within the bounds");
        zoom(ui, &send, None);
        assert_eq!(a.level.get_untracked(), 0.0);
        ui.take(&json!({"type": "setting_refused", "key": "window.zoomLevel", "error": "read-only"}), &send);
        assert_eq!(a.level.get_untracked(), -5.0, "a zoom not kept goes back to the one the settings say");
    }

    #[test]
    fn the_colour_settings_change_the_window_s_colours_at_once() {
        let (ui, send, told) = window();
        said(ui, &send, json!({"codeArena.accessibility.colorBlindColors": true, "codeArena.accessibility.contrast": "more"}));
        let c = ui.colors.get_untracked();
        assert_eq!(c.gutter_added, Color::rgb8(0x00, 0x72, 0xB2));
        assert_eq!(c.desc, c.fg);
        assert!(crate::commands::run(ui, &send, TOGGLE_COLOR_BLIND));
        let last = told.borrow().last().cloned().expect("a setting kept");
        assert_eq!(last["key"], "codeArena.accessibility.colorBlindColors");
        assert_eq!(last["value"], false);
        said(ui, &send, json!({}));
        assert_ne!(ui.colors.get_untracked().gutter_added, Color::rgb8(0x00, 0x72, 0xB2));
    }

    fn contrast(a: Color, b: Color) -> f64 {
        ca_themes::contrast(&hex_of(a), &hex_of(b)).unwrap_or(0.0)
    }

    #[test]
    fn colour_blind_colours_are_blue_and_orange_and_readable_in_every_theme() {
        for t in ca_themes::built_in() {
            let c = adjusted(Colors::of(&t), Seeing { safe: true, ..Seeing::default() });
            assert_eq!(c.gutter_added, Color::rgb8(0x00, 0x72, 0xB2), "{}", t.id);
            assert_eq!(c.gutter_deleted, Color::rgb8(0xD5, 0x5E, 0x00), "{}", t.id);
            for (name, fg) in [("added", c.added_fg), ("removed", c.removed_fg), ("passed", c.ok)] {
                assert!(fg.b > fg.g || fg.r > fg.g, "{name} in {} is neither blue nor orange", t.id);
                let ground = opaque(c.side_bg, if c.dark { Color::BLACK } else { Color::WHITE });
                assert!(contrast(fg, ground) >= 4.4, "{name} in {} reads {:.2}", t.id, contrast(fg, ground));
            }
        }
    }

    #[test]
    fn more_contrast_gives_secondary_text_the_texts_colour_and_borders_that_stand_out() {
        let all = ca_themes::built_in();
        let modern = Colors::of(&ca_themes::chosen(&all, "dark-modern", true));
        let c = adjusted(modern, Seeing { more: true, ..Seeing::default() });
        assert_eq!(c.desc, c.fg);
        assert_eq!(c.tab_inactive_fg, c.tab_active_fg);
        assert_eq!(c.focus, c.fg);
        assert!(contrast(c.panel_border, opaque(c.editor_bg, Color::BLACK)) >= 3.0);
        assert!(contrast(c.line_number, opaque(c.editor_bg, Color::BLACK)) >= 4.4);
        assert_eq!(adjusted(modern, Seeing::default()), modern);
        let strong = adjusted(modern, Seeing { strong: true, ..Seeing::default() });
        assert_eq!(strong.focus, modern.fg);
        assert_eq!(strong.desc, modern.desc);
    }
}
