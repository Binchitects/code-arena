//! Colour themes in VS Code's format: `name`, `type` (dark, light, hc, hcLight), `colors` by VS Code's ids and
//! `tokenColors`. The built-in ones are the JSON files beside this crate (the browser page reads the same files);
//! [`palette`] gives the colours both faces draw with, each from a theme's colour or its fallbacks.

use serde_json::Value;

const BUILT_IN: &[(&str, &str)] = &[
    ("dark-modern", include_str!("../themes/dark-modern.json")),
    ("light-modern", include_str!("../themes/light-modern.json")),
    ("dark-plus", include_str!("../themes/dark-plus.json")),
    ("light-plus", include_str!("../themes/light-plus.json")),
    ("hc-black", include_str!("../themes/hc-black.json")),
    ("hc-light", include_str!("../themes/hc-light.json")),
    ("monokai", include_str!("../themes/monokai.json")),
    ("solarized-dark", include_str!("../themes/solarized-dark.json")),
    ("solarized-light", include_str!("../themes/solarized-light.json")),
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
    BUILT_IN
        .iter()
        .filter_map(|(id, text)| serde_json::from_str(text).ok().map(|json| Theme { id: id.to_string(), json }))
        .collect()
}

/// The theme a choice names (its id or name, any case); "" or none found: Dark Modern, or Light Modern when the
/// system is light.
pub fn chosen(all: &[Theme], choice: &str, system_dark: bool) -> Theme {
    let c = choice.trim().to_lowercase();
    if !c.is_empty()
        && let Some(t) = all.iter().find(|t| t.id.to_lowercase() == c || t.name().to_lowercase() == c)
    {
        return t.clone();
    }
    let fallback = if system_dark { "dark-modern" } else { "light-modern" };
    built_in()
        .into_iter()
        .find(|t| t.id == fallback)
        .unwrap_or_else(|| Theme { id: fallback.into(), json: Value::Null })
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
        assert_eq!(all.len(), 9);
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
    fn a_theme_with_few_colours_takes_the_rest_from_its_type() {
        let t = Theme {
            id: "mine".into(),
            json: serde_json::json!({"type": "light", "colors": {"editor.background": "#fafafa", "x": "red"}}),
        };
        let p = palette(&t);
        assert_eq!((p.bg.as_str(), p.panel.as_str()), ("#fafafa", "#F8F8F8"));
    }
}
