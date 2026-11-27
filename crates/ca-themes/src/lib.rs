//! Colour themes in VS Code's format: `name`, `type` (dark, light, hc, hcLight), `colors` by VS Code's ids and
//! `tokenColors`. The built-in ones are the JSON files beside this crate (the browser page reads the same files);
//! [`palette`] gives the colours both faces draw with, each from a theme's colour or its fallbacks.
//!
//! Arena's own themes (Code Arena, Sakura, Midnight, Copper, Forest, Ocean, Lavender, each dark and light) and the
//! VS Code-named ones made from the old Code Arena's palettes (One Dark Pro, Dracula, GitHub, Nord, Monokai Light,
//! Visual Studio's) are written from its colours, their code colours made 4.5:1 on the editor and on the cursor's
//! line. A name none of them has is read as the one it stands for ([`alias`]), and a theme in force has its line
//! numbers, suggestions and links raised to 4.5:1 ([`readable`]).

use std::sync::OnceLock;

use serde_json::Value;

/// Arena's own first (each dark, then light), then VS Code's, then Visual Studio's.
const BUILT_IN: &[(&str, &str)] = &[
    ("code-arena-dark", include_str!("../themes/code-arena-dark.json")),
    ("code-arena-light", include_str!("../themes/code-arena-light.json")),
    ("sakura-dark", include_str!("../themes/sakura-dark.json")),
    ("sakura-light", include_str!("../themes/sakura-light.json")),
    ("midnight-dark", include_str!("../themes/midnight-dark.json")),
    ("midnight-light", include_str!("../themes/midnight-light.json")),
    ("copper-dark", include_str!("../themes/copper-dark.json")),
    ("copper-light", include_str!("../themes/copper-light.json")),
    ("forest-dark", include_str!("../themes/forest-dark.json")),
    ("forest-light", include_str!("../themes/forest-light.json")),
    ("ocean-dark", include_str!("../themes/ocean-dark.json")),
    ("ocean-light", include_str!("../themes/ocean-light.json")),
    ("lavender-dark", include_str!("../themes/lavender-dark.json")),
    ("lavender-light", include_str!("../themes/lavender-light.json")),
    ("dark-modern", include_str!("../themes/dark-modern.json")),
    ("light-modern", include_str!("../themes/light-modern.json")),
    ("dark-plus", include_str!("../themes/dark-plus.json")),
    ("light-plus", include_str!("../themes/light-plus.json")),
    ("hc-black", include_str!("../themes/hc-black.json")),
    ("hc-light", include_str!("../themes/hc-light.json")),
    ("monokai", include_str!("../themes/monokai.json")),
    ("monokai-light", include_str!("../themes/monokai-light.json")),
    ("solarized-dark", include_str!("../themes/solarized-dark.json")),
    ("solarized-light", include_str!("../themes/solarized-light.json")),
    ("one-dark-pro", include_str!("../themes/one-dark-pro.json")),
    ("one-light", include_str!("../themes/one-light.json")),
    ("dracula", include_str!("../themes/dracula.json")),
    ("alucard", include_str!("../themes/alucard.json")),
    ("github-dark", include_str!("../themes/github-dark.json")),
    ("github-light", include_str!("../themes/github-light.json")),
    ("nord", include_str!("../themes/nord.json")),
    ("nord-light", include_str!("../themes/nord-light.json")),
    ("visual-studio-dark", include_str!("../themes/visual-studio-dark.json")),
    ("visual-studio-light", include_str!("../themes/visual-studio-light.json")),
];

/// A theme: its id, and its VS Code JSON.
#[derive(Debug, Clone)]
pub struct Theme {
    pub id: String,
    pub json: Value,
}

impl Theme {
    pub fn name(&self) -> &str {
        self.json["name"].as_str().unwrap_or(&self.id)
    }

    pub fn kind(&self) -> &str {
        self.json["type"].as_str().unwrap_or("dark")
    }

    pub fn is_dark(&self) -> bool {
        matches!(self.kind(), "dark" | "hc")
    }
}

/// The built-in themes, in their order.
pub fn built_in() -> Vec<Theme> {
    // Read once: a theme is looked up each time one is applied or previewed.
    static READ: OnceLock<Vec<Theme>> = OnceLock::new();
    READ.get_or_init(|| {
        BUILT_IN
            .iter()
            .filter_map(|(id, text)| serde_json::from_str(text).ok().map(|json| Theme { id: id.to_string(), json }))
            .collect()
    })
    .clone()
}

/// Each theme's family: its name in the theme button's list, and its dark and light modes' names. The button's
/// Light and Dark go between the two, and a family chosen there makes them the preferred ones.
pub const FAMILIES: &[(&str, &str, &str)] = &[
    ("Code Arena", "Code Arena Dark", "Code Arena Light"),
    ("Sakura", "Sakura Dark", "Sakura Light"),
    ("Midnight", "Midnight Dark", "Midnight Light"),
    ("Copper", "Copper Dark", "Copper Light"),
    ("Forest", "Forest Dark", "Forest Light"),
    ("Ocean", "Ocean Dark", "Ocean Light"),
    ("Lavender", "Lavender Dark", "Lavender Light"),
    ("VS Code Modern", "Dark Modern", "Light Modern"),
    ("VS Code", "Dark+", "Light+"),
    ("High Contrast", "Dark High Contrast", "Light High Contrast"),
    ("Monokai", "Monokai", "Monokai Light"),
    ("Solarized", "Solarized Dark", "Solarized Light"),
    ("One Dark Pro", "One Dark Pro", "One Light"),
    ("Dracula", "Dracula", "Alucard"),
    ("GitHub", "GitHub Dark", "GitHub Light"),
    ("Nord", "Nord", "Nord Light"),
    ("Visual Studio", "Dark (Visual Studio)", "Light (Visual Studio)"),
];

/// The family a theme's name is of: its label, and its dark and its light mode's names.
pub fn family_of(name: &str) -> Option<(&'static str, &'static str, &'static str)> {
    FAMILIES.iter().copied().find(|(_, d, l)| d.eq_ignore_ascii_case(name) || l.eq_ignore_ascii_case(name))
}

/// Other names for the themes here, the old Code Arena's list: VS Code's names in settings.json ("Default Dark
/// Modern"), older ones and the extensions' ("Dark+ (default dark)", "GitHub Dark Default", "Atom One Light"). Each
/// is a list of ways to match, every word of one found in the name in that order; the first that matches wins, so a
/// light one comes before its dark one.
type Ways = &'static [&'static [&'static str]];
const OTHER_NAMES: &[(Ways, &str)] = &[
    (&[&["high contrast", "light"], &["light", "high contrast"]], "Light High Contrast"),
    (&[&["high contrast"]], "Dark High Contrast"),
    (&[&["light+"], &["light (visual studio code)"]], "Light+"),
    (&[&["dark+"], &["dark (visual studio code)"]], "Dark+"),
    (&[&["light modern"]], "Light Modern"),
    (&[&["dark modern"]], "Dark Modern"),
    (&[&["monokai", "light"]], "Monokai Light"),
    (&[&["monokai"]], "Monokai"),
    (&[&["solarized", "light"]], "Solarized Light"),
    (&[&["solarized"]], "Solarized Dark"),
    (&[&["one light"]], "One Light"),
    (&[&["one dark"]], "One Dark Pro"),
    (&[&["alucard"], &["dracula", "light"]], "Alucard"),
    (&[&["dracula"]], "Dracula"),
    (&[&["github", "light"]], "GitHub Light"),
    (&[&["github"]], "GitHub Dark"),
    (&[&["nord", "light"], &["snow storm"], &["snowstorm"]], "Nord Light"),
    (&[&["nord"]], "Nord"),
    (&[&["arena", "light"]], "Code Arena Light"),
    (&[&["arena"]], "Code Arena Dark"),
    (&[&["sakura", "light"]], "Sakura Light"),
    (&[&["sakura"]], "Sakura Dark"),
    (&[&["midnight", "light"]], "Midnight Light"),
    (&[&["midnight"]], "Midnight Dark"),
    (&[&["copper", "light"]], "Copper Light"),
    (&[&["copper"]], "Copper Dark"),
    (&[&["forest", "light"]], "Forest Light"),
    (&[&["forest"]], "Forest Dark"),
    (&[&["ocean", "light"]], "Ocean Light"),
    (&[&["ocean"]], "Ocean Dark"),
    (&[&["lavender", "light"]], "Lavender Light"),
    (&[&["lavender"]], "Lavender Dark"),
];

/// Whether every word is in `name`, each after the one before.
fn has_in_order(name: &str, words: &[&str]) -> bool {
    let mut rest = name;
    words.iter().all(|w| match rest.find(w) {
        Some(at) => {
            rest = &rest[at + w.len()..];
            true
        }
        None => false,
    })
}

/// The name of this crate's theme another name stands for, if one does.
pub fn alias(name: &str) -> Option<&'static str> {
    let n = name.trim().to_lowercase();
    OTHER_NAMES.iter().find(|(ways, _)| ways.iter().any(|w| has_in_order(&n, w))).map(|(_, to)| *to)
}

/// The theme a name or id is (any case), or the one an older or an extension's name stands for.
pub fn named(all: &[Theme], choice: &str) -> Option<Theme> {
    let c = choice.trim().to_lowercase();
    if c.is_empty() {
        return None;
    }
    let find = |c: &str| all.iter().find(|t| t.id.to_lowercase() == c || t.name().to_lowercase() == c).cloned();
    find(&c).or_else(|| alias(&c).and_then(|to| find(&to.to_lowercase())))
}

/// The theme a choice names (its id or name, any case, or another name of it), its hard-to-read colours raised
/// ([`readable`]). "": Dark Modern, or Light Modern when the system is light; a name none is: the nearest, Code
/// Arena Light when it says light, else Code Arena Dark.
pub fn chosen(all: &[Theme], choice: &str, system_dark: bool) -> Theme {
    let mut t = named(all, choice).unwrap_or_else(|| {
        let c = choice.trim().to_lowercase();
        let fallback = match () {
            _ if c.is_empty() && system_dark => "dark-modern",
            _ if c.is_empty() => "light-modern",
            _ if c.contains("light") => "code-arena-light",
            _ => "code-arena-dark",
        };
        built_in()
            .into_iter()
            .find(|t| t.id == fallback)
            .unwrap_or_else(|| Theme { id: fallback.into(), json: Value::Null })
    });
    readable(&mut t);
    t
}

fn rgb(s: &str) -> Option<[f64; 3]> {
    let h = s.strip_prefix('#')?;
    let n = |i: usize, w: usize| {
        u8::from_str_radix(h.get(i..i + w)?, 16).ok().map(|v| f64::from(if w == 1 { v * 17 } else { v }))
    };
    match h.len() {
        3 => Some([n(0, 1)?, n(1, 1)?, n(2, 1)?]),
        6 => Some([n(0, 2)?, n(2, 2)?, n(4, 2)?]),
        _ => None,
    }
}

fn luminance(c: [f64; 3]) -> f64 {
    let l = |x: f64| {
        let x = x / 255.0;
        if x <= 0.04045 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * l(c[0]) + 0.7152 * l(c[1]) + 0.0722 * l(c[2])
}

/// The contrast of two opaque colours, WCAG's: 1 to 21.
pub fn contrast(a: &str, b: &str) -> Option<f64> {
    let (x, y) = (luminance(rgb(a)?), luminance(rgb(b)?));
    Some((x.max(y) + 0.05) / (x.min(y) + 0.05))
}

/// A colour moved towards white (on a dark ground) or black (light) in small steps until it is 4.5:1 on every one
/// of `grounds`: as little as it needs. One already 4.5:1, or with an alpha of its own or on a ground that has one,
/// is as it was.
pub fn raised(colour: &str, grounds: &[&str], dark: bool) -> String {
    let (Some(from), true) = (rgb(colour), grounds.iter().all(|g| rgb(g).is_some())) else { return colour.into() };
    let reads = |c: [f64; 3]| {
        let hex = format!("#{:02x}{:02x}{:02x}", c[0].round() as u8, c[1].round() as u8, c[2].round() as u8);
        grounds.iter().all(|g| contrast(&hex, g).is_some_and(|r| r >= 4.5)).then_some(hex)
    };
    if reads(from).is_some() {
        return colour.into();
    }
    (1..=50)
        .find_map(|i| {
            let t = f64::from(i) / 50.0;
            reads(from.map(|x| if dark { x + (255.0 - x) * t } else { x * (1.0 - t) }))
        })
        .unwrap_or_else(|| (if dark { "#ffffff" } else { "#000000" }).into())
}

/// The colours read as text that a theme may give under 4.5:1 on their ground (WCAG AA), each raised just enough:
/// the line numbers on the gutter, a suggestion's text and its typed letters on the suggestions, a list's typed
/// letters on the side bar, and links on the editor and the side bar. One the theme leaves out is VS Code's default
/// for it (or the colour it falls back to), raised and set.
pub fn readable(t: &mut Theme) {
    let dark = t.is_dark();
    let Some(colors) = t.json.get_mut("colors").and_then(Value::as_object_mut) else { return };
    let get = |colors: &serde_json::Map<String, Value>, ids: &[&str], d: &str, l: &str| -> String {
        ids.iter()
            .find_map(|id| colors.get(*id).and_then(Value::as_str).filter(|v| is_colour(v)).map(str::to_string))
            .unwrap_or_else(|| (if dark { d } else { l }).to_string())
    };
    let editor = get(colors, &["editor.background"], "#1E1E1E", "#FFFFFF");
    let gutter = get(colors, &["editorGutter.background"], &editor, &editor);
    let side = get(colors, &["sideBar.background"], &editor, &editor);
    let widget = get(colors, &["editorWidget.background"], "#252526", "#F3F3F3");
    let suggest = get(colors, &["editorSuggestWidget.background"], &widget, &widget);
    let text = get(colors, &["editor.foreground", "foreground"], "#D4D4D4", "#000000");
    let rows: [(&[&str], &str, &str, Vec<&str>); 5] = [
        (&["editorLineNumber.foreground"], "#858585", "#237893", vec![&gutter]),
        (&["editorSuggestWidget.foreground", "editor.foreground"], &text, &text, vec![&suggest]),
        (
            &["editorSuggestWidget.highlightForeground", "list.highlightForeground"],
            "#2AAAFF",
            "#0066BF",
            vec![&suggest],
        ),
        (&["list.highlightForeground"], "#2AAAFF", "#0066BF", vec![&side]),
        (&["textLink.foreground"], "#3794FF", "#006AB1", vec![&editor, &side]),
    ];
    let mut set = Vec::new();
    for (ids, d, l, grounds) in rows {
        let was = get(colors, ids, d, l);
        let now = raised(&was, &grounds, dark);
        if now != was || !colors.contains_key(ids[0]) {
            set.push((ids[0], now));
        }
    }
    for (id, v) in set {
        colors.insert(id.into(), Value::String(v));
    }
}

fn is_colour(s: &str) -> bool {
    s.len() >= 4 && s.len() <= 9 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit())
}

/// The colours both faces draw with (as the page's CSS variables), `#RRGGBB` or `#RRGGBBAA`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    pub bg: String,
    pub panel: String,
    pub text: String,
    pub muted: String,
    pub line: String,
    pub accent: String,
    pub button: String,
    pub button_text: String,
    pub hover: String,
    pub selection: String,
    pub code: String,
    pub input: String,
    pub danger: String,
    pub ok: String,
    pub activity: String,
    pub activity_fg: String,
    pub activity_border: String,
    pub title: String,
    pub title_fg: String,
    pub status: String,
    pub status_fg: String,
    pub tab: String,
    pub tab_active: String,
    pub tab_border_top: String,
    pub terminal: String,
}

/// A theme's palette: each colour from the first of its VS Code ids the theme has, else its type's default (the
/// same rules as the page's `cssVars`).
pub fn palette(t: &Theme) -> Palette {
    let dark = t.is_dark();
    let pick = |ids: &[&str], d: &str, l: &str| -> String {
        ids.iter()
            .find_map(|id| t.json["colors"][*id].as_str().filter(|v| is_colour(v)).map(str::to_string))
            .unwrap_or_else(|| (if dark { d } else { l }).to_string())
    };
    let text = pick(&["foreground", "editor.foreground"], "#CCCCCC", "#3B3B3B");
    Palette {
        bg: pick(&["editor.background"], "#1F1F1F", "#FFFFFF"),
        panel: pick(&["sideBar.background", "panel.background"], "#181818", "#F8F8F8"),
        muted: pick(&["descriptionForeground"], "#9D9D9D", "#6F6F6F"),
        line: pick(&["panel.border", "sideBar.border", "widget.border", "contrastBorder"], "#2B2B2B", "#E5E5E5"),
        accent: pick(&["focusBorder", "button.background"], "#0078D4", "#005FB8"),
        button: pick(&["button.background"], "#0078D4", "#005FB8"),
        button_text: pick(&["button.foreground"], "#FFFFFF", "#FFFFFF"),
        hover: pick(&["list.hoverBackground", "input.background"], "#2A2D2E", "#F2F2F2"),
        selection: pick(&["list.activeSelectionBackground"], "#37373D", "#E8E8E8"),
        code: pick(&["textCodeBlock.background", "editorWidget.background"], "#2B2B2B", "#F8F8F8"),
        input: pick(&["input.background"], "#313131", "#FFFFFF"),
        danger: pick(&["errorForeground"], "#F85149", "#C72E0F"),
        ok: pick(&["testing.iconPassed", "terminal.ansiGreen"], "#73C991", "#388A34"),
        activity: pick(&["activityBar.background"], "#181818", "#F8F8F8"),
        activity_fg: pick(&["activityBar.foreground"], "#D7D7D7", "#1F1F1F"),
        activity_border: pick(&["activityBar.activeBorder", "focusBorder"], "#0078D4", "#005FB8"),
        title: pick(&["titleBar.activeBackground"], "#181818", "#F8F8F8"),
        title_fg: pick(&["titleBar.activeForeground"], &text, &text),
        status: pick(&["statusBar.background"], "#181818", "#F8F8F8"),
        status_fg: pick(&["statusBar.foreground"], "#CCCCCC", "#3B3B3B"),
        tab: pick(&["tab.inactiveBackground", "editorGroupHeader.tabsBackground"], "#181818", "#F8F8F8"),
        tab_active: pick(&["tab.activeBackground", "editor.background"], "#1F1F1F", "#FFFFFF"),
        tab_border_top: pick(&["tab.activeBorderTop", "focusBorder"], "#0078D4", "#005FB8"),
        terminal: pick(&["terminal.background", "panel.background"], "#181818", "#F8F8F8"),
        text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_built_in_theme_reads_and_gives_every_colour() {
        let all = built_in();
        assert_eq!(all.len(), 34);
        for t in &all {
            let p = palette(t);
            for c in [&p.bg, &p.panel, &p.text, &p.accent, &p.status, &p.terminal] {
                assert!(is_colour(c), "{} {c}", t.name());
            }
        }
        assert_eq!(palette(&chosen(&all, "", true)).bg, "#1F1F1F");
        assert_eq!(chosen(&all, "Monokai", true).id, "monokai");
        assert_eq!(chosen(&all, "", false).id, "light-modern");
    }

    #[test]
    fn every_family_is_two_themes_here_one_dark_one_light() {
        let all = built_in();
        for (label, d, l) in FAMILIES {
            let (dark, light) = (named(&all, d).expect(d), named(&all, l).expect(l));
            assert!(dark.is_dark() && !light.is_dark(), "{d} {l}");
            assert_eq!(family_of(&light.name().to_uppercase()), Some((*label, *d, *l)));
        }
        let mut names: Vec<&str> = all.iter().map(Theme::name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), all.len());
    }

    #[test]
    fn older_and_extension_names_read_as_the_theme_they_stand_for() {
        let all = built_in();
        let to = |n: &str| chosen(&all, n, true).name().to_string();
        assert_eq!(to("Default Dark Modern"), "Dark Modern");
        assert_eq!(to("Dark+ (default dark)"), "Dark+");
        assert_eq!(to("Default Light+"), "Light+");
        assert_eq!(to("Default High Contrast Light"), "Light High Contrast");
        assert_eq!(to("Default High Contrast"), "Dark High Contrast");
        assert_eq!(to("GitHub Dark Default"), "GitHub Dark");
        assert_eq!(to("GitHub Light Default"), "GitHub Light");
        assert_eq!(to("Atom One Light"), "One Light");
        assert_eq!(to("Dracula Soft"), "Dracula");
        assert_eq!(to("Solarized Dark (Bright)"), "Solarized Dark");
        assert_eq!(to("Nord Snowstorm"), "Nord Light");
        assert_eq!(to("sakura-light"), "Sakura Light");
        assert_eq!(to("code arena light"), "Code Arena Light");
        // None is near: Code Arena's, light when the name says so.
        assert_eq!(to("Tokyo Night"), "Code Arena Dark");
        assert_eq!(to("Quiet Light"), "Code Arena Light");
    }

    #[test]
    fn line_numbers_suggestions_and_links_are_4_5_to_1_in_every_theme() {
        let all = built_in();
        for t in &all {
            let t = chosen(&all, &t.id, true);
            let c = &t.json["colors"];
            let s = |id: &str| c[id].as_str().unwrap_or_else(|| panic!("{} {id}", t.name())).to_string();
            let editor = s("editor.background");
            let at_least = |fg: &str, bg: &str| {
                let r = contrast(fg, bg).unwrap_or(21.0);
                assert!(r >= 4.5, "{}: {fg} on {bg} is {r:.2}", t.name());
            };
            at_least(&s("editorLineNumber.foreground"), c["editorGutter.background"].as_str().unwrap_or(&editor));
            at_least(&s("textLink.foreground"), &editor);
            let widget = c["editorSuggestWidget.background"].as_str().or(c["editorWidget.background"].as_str());
            if let Some(w) = widget {
                at_least(&s("editorSuggestWidget.highlightForeground"), w);
                at_least(&s("editorSuggestWidget.foreground"), w);
            }
        }
    }

    #[test]
    fn a_colour_is_raised_only_as_much_as_it_needs() {
        assert_eq!(raised("#ffffff", &["#000000"], true), "#ffffff");
        let up = raised("#444444", &["#1f1f1f"], true);
        assert!(contrast(&up, "#1f1f1f").unwrap_or(0.0) >= 4.5);
        assert!(contrast(&up, "#1f1f1f").unwrap_or(0.0) < 5.0, "{up}");
        let down = raised("#aaaaaa", &["#ffffff"], false);
        assert!(contrast(&down, "#ffffff").unwrap_or(0.0) >= 4.5);
        // A colour with its own alpha is left as it is.
        assert_eq!(raised("#44444480", &["#1f1f1f"], true), "#44444480");
    }

    #[test]
    fn a_theme_with_few_colours_takes_the_rest_from_its_type() {
        let t = Theme {
            id: "mine".into(),
            json: serde_json::json!({"type": "light", "colors": {"editor.background": "#fafafa", "x": "red"}}),
        };
        let p = palette(&t);
        assert_eq!((p.bg.as_str(), p.panel.as_str()), ("#fafafa", "#F8F8F8"));
    }
}
