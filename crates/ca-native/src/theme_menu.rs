//! The colour theme in force, the Color Theme pick and the activity bar's theme button.
//!
//! The theme in force is the one previewed in the pick, else one just chosen until the session says it is kept,
//! else with `window.autoDetectColorScheme` the preferred dark or light one as the system is
//! (`workbench.preferredDarkColorTheme`, `workbench.preferredLightColorTheme`), else `workbench.colorTheme`
//! ([`in_force`]). The window applies it ([`theme`]); a change of any of these applies it again.
//!
//! **Preferences: Color Theme** (Ctrl+K Ctrl+T, and the old Code Arena's Ctrl+K Ctrl+Alt+T) lists Follow the System,
//! then the light, dark and high contrast themes, Code Arena's and then yours (a theme from a themes/ folder). The
//! theme under the arrow keys is shown at once; Escape shows the one before again. Choosing a theme turns following
//! the system off, as in the old Code Arena.
//!
//! The theme button at the activity bar's foot has Light, Dark and System, and the Color Theme submenu of the
//! families: choosing one makes its two modes the preferred ones (so System keeps it), and Light or Dark goes to the
//! same family's other mode. A setting the session refuses to keep (`setting_refused`) is said in a toast.

use floem::IntoView;
use floem::event::{Event, EventListener};
use floem::reactive::{Memo, RwSignal, SignalGet, SignalUpdate, SignalWith};
use floem::views::{Decorators, empty};
use serde_json::{Value, json};

use crate::commands::{add, command};
use crate::settings;
use crate::state::{Send, Ui};
use crate::widgets::{popup, quick, toast};

/// The theme button's command: its menu, beside the button.
pub const THEME_BUTTON: &str = "codeArena.themeMenu";
/// VS Code's Preferences: Toggle between Light/Dark Themes.
pub const TOGGLE_LIGHT_DARK: &str = "workbench.action.toggleLightDarkThemes";
/// The pick's row that turns following the system on.
const FOLLOW: &str = "@follow";

/// What the theme in force is made from, besides the settings.
#[derive(Clone, Copy)]
pub struct Follow {
    /// Whether the system is dark, once the window has been told (none: not told; taken as dark).
    system: RwSignal<Option<bool>>,
    /// The theme shown while the pick is open on it.
    preview: RwSignal<Option<String>>,
    /// The theme chosen, shown until the settings the session says give it.
    pending: RwSignal<Option<String>>,
    auto: Memo<Value>,
    dark: Memo<Value>,
    light: Memo<Value>,
}

impl crate::ext::State for Follow {
    fn new(ui: Ui) -> Follow {
        Follow {
            system: ui.scope.create_rw_signal(None),
            preview: ui.scope.create_rw_signal(None),
            pending: ui.scope.create_rw_signal(None),
            auto: settings::memo(ui, "window.autoDetectColorScheme", || None),
            dark: settings::memo(ui, "workbench.preferredDarkColorTheme", || None),
            light: settings::memo(ui, "workbench.preferredLightColorTheme", || None),
        }
    }
}

/// The settings that say which theme is in force, as they are now.
#[derive(Clone, Debug, PartialEq)]
pub struct Said {
    pub auto: bool,
    pub system_dark: Option<bool>,
    pub setting: String,
    pub dark: String,
    pub light: String,
}

/// The name of the theme in force: the one previewed, else the one chosen and not yet kept, else the preferred one as
/// the system is (dark when the window has not been told), else the setting.
pub fn in_force(preview: Option<&str>, pending: Option<&str>, said: &Said) -> String {
    if let Some(p) = preview.or(pending) {
        return p.to_string();
    }
    if said.auto {
        return if said.system_dark.unwrap_or(true) { said.dark.clone() } else { said.light.clone() };
    }
    said.setting.clone()
}

fn said(ui: Ui) -> Said {
    let s = ui.state::<Follow>();
    let text = |v: Value| v.as_str().unwrap_or("").to_string();
    Said {
        auto: s.auto.get_untracked().as_bool().unwrap_or(false),
        system_dark: s.system.get_untracked(),
        setting: ui.choice.get_untracked(),
        dark: text(s.dark.get_untracked()),
        light: text(s.light.get_untracked()),
    }
}

/// The theme a name gives among the window's themes, as the window would draw it.
fn of(ui: Ui, name: &str) -> ca_themes::Theme {
    let dark = ui.state::<Follow>().system.get_untracked().unwrap_or(true);
    ui.themes.with_untracked(|all| ca_themes::chosen(all, name, dark))
}

/// The theme in force now, with its hard-to-read colours raised: what the window draws with. Not followed.
pub fn theme(ui: Ui) -> ca_themes::Theme {
    let s = ui.state::<Follow>();
    let (preview, pending) = (s.preview.get_untracked(), s.pending.get_untracked());
    of(ui, &in_force(preview.as_deref(), pending.as_deref(), &said(ui)))
}

/// The theme the settings give, leaving out one previewed or chosen.
fn settled(ui: Ui) -> ca_themes::Theme {
    of(ui, &in_force(None, None, &said(ui)))
}

fn set(send: &Send, key: &str, value: Value) {
    send(json!({"type": "set_setting", "key": key, "value": value}));
}

/// A theme chosen: kept as `workbench.colorTheme` (by its name, as VS Code keeps it), following the system turned
/// off after it (so the window never shows the setting from before in between), and shown until the session says so.
pub(crate) fn choose(ui: Ui, send: &Send, name: &str) {
    let s = ui.state::<Follow>();
    let was = said(ui);
    if was.auto || of(ui, name).id != of(ui, &was.setting).id {
        if s.pending.get_untracked().as_deref() != Some(name) {
            s.pending.set(Some(name.to_string()));
        }
        set(send, "workbench.colorTheme", json!(name));
    }
    if was.auto {
        set(send, "window.autoDetectColorScheme", json!(false));
    }
}

/// Following the system turned on, with a family's two modes as the preferred ones when one is given.
fn follow(ui: Ui, send: &Send, pair: Option<(&str, &str)>) {
    let s = ui.state::<Follow>();
    let mut now = said(ui);
    if let Some((d, l)) = pair {
        if now.dark != d {
            set(send, "workbench.preferredDarkColorTheme", json!(d));
            now.dark = d.into();
        }
        if now.light != l {
            set(send, "workbench.preferredLightColorTheme", json!(l));
            now.light = l.into();
        }
    }
    if !now.auto {
        set(send, "window.autoDetectColorScheme", json!(true));
        now.auto = true;
    }
    let shown = in_force(None, None, &now);
    if of(ui, &shown).id != settled(ui).id {
        s.pending.set(Some(shown));
    }
}

/// The pick's rows: Follow the System, then the light, dark and high contrast themes, Code Arena's before yours.
pub fn rows(all: &[ca_themes::Theme], current: Option<&str>, follow: &str) -> Vec<quick::Item> {
    let mut out = vec![quick::item(FOLLOW, "Follow the System").description(follow)];
    for yours in [false, true] {
        for (kinds, group) in [
            (&["light"][..], "light themes"),
            (&["dark"][..], "dark themes"),
            (&["hc", "hcLight"][..], "high contrast themes"),
        ] {
            let group = if yours { format!("your {group}") } else { group.to_string() };
            for t in all.iter().filter(|t| t.json["from"].is_string() == yours && kinds.contains(&t.kind())) {
                let mark = if Some(t.id.as_str()) == current { "current" } else { "" };
                out.push(quick::item(&t.id, t.name()).description(mark).group(&group));
            }
        }
    }
    out
}

/// Preferences: Color Theme.
pub fn pick(ui: Ui, send: &Send) {
    let s = ui.state::<Follow>();
    let now = said(ui);
    let current = settled(ui).id;
    let both = format!("{} or {}", now.dark, now.light);
    let items = ui.themes.with_untracked(|all| rows(all, (!now.auto).then_some(current.as_str()), &both));
    let send = send.clone();
    quick::pick("Select Color Theme (Up/Down Keys to Preview)")
        .placeholder("Select Color Theme (Up/Down Keys to Preview)")
        .items(items)
        .active(if now.auto { FOLLOW } else { &current })
        .on_active(move |item| {
            let shown = item.map(|i| {
                if i.id == FOLLOW { in_force(None, None, &Said { auto: true, ..said(ui) }) } else { i.id.clone() }
            });
            if s.preview.get_untracked() != shown {
                s.preview.set(shown);
            }
        })
        .open(ui, move |picked| {
            // The one taken is held before the preview is let go, so the window does not show another between.
            match picked {
                Some(i) if i.id == FOLLOW => follow(ui, &send, None),
                Some(i) => {
                    let name = of(ui, &i.id).name().to_string();
                    choose(ui, &send, &name);
                }
                None => {}
            }
            if s.preview.get_untracked().is_some() {
                s.preview.set(None);
            }
        });
}

/// The theme button's Light or Dark: the same family's mode, or Code Arena's for a theme of no family.
fn mode(ui: Ui, send: &Send, dark: bool) {
    let now = theme(ui);
    let (_, d, l) = ca_themes::family_of(now.name()).unwrap_or(ca_themes::FAMILIES[0]);
    choose(ui, send, if dark { d } else { l });
}

/// The theme button's menu.
pub fn menu(ui: Ui, send: &Send, at: popup::At) {
    let now = said(ui);
    let shown = theme(ui);
    let dark = shown.is_dark();
    let family = ca_themes::family_of(shown.name());
    let pair = family.map(|(_, d, l)| (d, l));
    let s1 = send.clone();
    let s2 = send.clone();
    let s3 = send.clone();
    let mut families: Vec<popup::Item> = ca_themes::FAMILIES
        .iter()
        .map(|&(label, d, l)| {
            let send = send.clone();
            let auto = now.auto;
            popup::Item::Entry(
                popup::entry(label, move || {
                    if auto {
                        follow(ui, &send, Some((d, l)));
                    } else {
                        set(&send, "workbench.preferredDarkColorTheme", json!(d));
                        set(&send, "workbench.preferredLightColorTheme", json!(l));
                        choose(ui, &send, if dark { d } else { l });
                    }
                })
                .radio(family.is_some_and(|f| f.0 == label)),
            )
        })
        .collect();
    let s4 = send.clone();
    families.push(popup::line());
    families.push(popup::Item::Entry(
        popup::entry("Browse Color Themes…", move || pick(ui, &s4))
            .keys(&crate::commands::keys_of(crate::workbench::SELECT_THEME)),
    ));
    let items = vec![
        popup::Item::Entry(popup::entry("Light", move || mode(ui, &s1, false)).radio(!now.auto && !dark)),
        popup::Item::Entry(popup::entry("Dark", move || mode(ui, &s2, true)).radio(!now.auto && dark)),
        popup::Item::Entry(popup::entry("System", move || follow(ui, &s3, pair)).radio(now.auto)),
        popup::line(),
        popup::Item::Entry(popup::submenu("Color Theme", families)),
    ];
    popup::open(ui, at, items);
}

/// Told by the window when the system's light or dark changes (and once as it opens, where the system says).
fn system_watch(ui: Ui) -> impl IntoView {
    empty().style(|s| s.absolute().size(0.0, 0.0)).on_event_cont(EventListener::ThemeChanged, move |e| {
        if let Event::ThemeChanged(t) = e {
            let dark = Some(matches!(t, floem::window::Theme::Dark));
            let s = ui.state::<Follow>();
            if s.system.get_untracked() != dark {
                s.system.set(dark);
            }
        }
    })
}

/// What it adds to the window's registries, once at start.
pub fn register(ui: Ui, _send: &Send) {
    add(command(crate::workbench::SELECT_THEME, "Preferences", "Color Theme", pick)
        .keys("Ctrl+K Ctrl+T")
        .keys("Ctrl+K Ctrl+Alt+T"));
    add(command(TOGGLE_LIGHT_DARK, "Preferences", "Toggle between Light/Dark Themes", |ui, send| {
        mode(ui, send, !theme(ui).is_dark())
    }));
    add(command(THEME_BUTTON, "Preferences", "Theme", |ui, send| {
        if let Some(button) = crate::activity_bar::foot_button() {
            menu(ui, send, popup::At::beside(button));
        }
    })
    .hidden());
    crate::layers::add(crate::layers::Place::Over, "theme.system", 0, |ui, _| system_watch(ui).into_any());
    // The theme applied again when what makes it changes: the settings, the system, the pick's preview, the theme
    // chosen. A theme chosen is let go once the settings give it.
    let s = ui.state::<Follow>();
    ui.scope.create_effect(move |_| {
        s.auto.with(|_| ());
        s.dark.with(|_| ());
        s.light.with(|_| ());
        s.system.with(|_| ());
        s.preview.with(|_| ());
        ui.choice.with(|_| ());
        ui.themes.with(|_| ());
        if let Some(p) = s.pending.get()
            && of(ui, &p).id == settled(ui).id
        {
            s.pending.set(None);
        }
        ui.apply_theme();
    });
}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    if m["type"] == "setting_refused" {
        let key = m["key"].as_str().unwrap_or("");
        let error = m["error"].as_str().unwrap_or("");
        toast::error(ui, &format!("Unable to write \"{key}\": {error}"));
        let s = ui.state::<Follow>();
        if key.contains("Theme") || key == "window.autoDetectColorScheme" {
            s.pending.update(|p| *p = None);
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(auto: bool, system_dark: Option<bool>) -> Said {
        Said { auto, system_dark, setting: "Monokai".into(), dark: "Nord".into(), light: "Nord Light".into() }
    }

    #[test]
    fn the_theme_in_force_is_the_preview_then_the_one_chosen_then_the_system_then_the_setting() {
        assert_eq!(in_force(None, None, &said(false, Some(false))), "Monokai");
        assert_eq!(in_force(None, None, &said(true, Some(false))), "Nord Light");
        assert_eq!(in_force(None, None, &said(true, Some(true))), "Nord");
        // A window the system has said nothing to is dark.
        assert_eq!(in_force(None, None, &said(true, None)), "Nord");
        assert_eq!(in_force(None, Some("Dracula"), &said(true, None)), "Dracula");
        assert_eq!(in_force(Some("Alucard"), Some("Dracula"), &said(false, None)), "Alucard");
    }

    #[test]
    fn the_pick_groups_light_dark_and_high_contrast_code_arenas_before_yours() {
        let mut all = ca_themes::built_in();
        all.push(ca_themes::Theme {
            id: "mine".into(),
            json: json!({"name": "Mine", "type": "light", "from": "/x/themes/mine.json"}),
        });
        let rows = rows(&all, Some("nord"), "Nord or Nord Light");
        assert_eq!((rows[0].id.as_str(), rows[0].label.as_str()), (FOLLOW, "Follow the System"));
        assert_eq!(rows[1].group, "light themes");
        let groups: Vec<&str> = rows[1..].iter().map(|r| r.group.as_str()).collect();
        let mut seen: Vec<&str> = groups.clone();
        seen.dedup();
        assert_eq!(seen, ["light themes", "dark themes", "high contrast themes", "your light themes"]);
        assert_eq!(rows.last().map(|r| r.label.as_str()), Some("Mine"));
        let marked: Vec<&str> = rows.iter().filter(|r| r.description == "current").map(|r| r.label.as_str()).collect();
        assert_eq!(marked, ["Nord"]);
        assert_eq!(rows.len(), all.len() + 1);
    }
}
