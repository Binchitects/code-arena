//! VS Code's icons: the Codicons font, drawn as text in its own family.

use floem::views::{Decorators, Label, label};

use crate::icons_table::CODICONS;

static CODICON: &[u8] = include_bytes!("../assets/codicon.ttf");

/// Gives the window's text the Codicons font. Once, before the first view.
pub fn load() {
    floem::text::FONT_SYSTEM.lock().db_mut().load_font_data(CODICON.to_vec());
}

/// VS Code's other names for some codicons (several names share a glyph, and the font names each glyph once), and
/// the names the window began with whose character is not the one the font's table gives (the same glyph under
/// another code point). Looked up before the table.
const ALIASES: [(&str, u32); 25] = [
    ("alert", 0xea6c),
    ("clock", 0xea82),
    ("console", 0xea85),
    ("file", 0xea7b),
    ("git-branch", 0xec6f),
    ("kebab-horizontal", 0xea7c),
    ("light-bulb", 0xea61),
    ("more", 0xea7c),
    ("pencil", 0xea73),
    ("plus", 0xea60),
    ("stop", 0xea87),
    ("symbol-constructor", 0xea8c),
    ("symbol-function", 0xea8c),
    ("symbol-module", 0xea8b),
    ("symbol-null", 0xea8f),
    ("symbol-number", 0xea90),
    ("symbol-object", 0xea8b),
    ("symbol-package", 0xea8b),
    ("symbol-string", 0xeb8d),
    ("symbol-struct", 0xea91),
    ("symbol-text", 0xea93),
    ("symbol-unit", 0xea96),
    ("symbol-value", 0xea95),
    ("trashcan", 0xea81),
    ("x", 0xea76),
];

/// The character drawn for a name the font does not have: "info".
const UNKNOWN: u32 = 0xea74;

fn code_of(name: &str) -> Option<u32> {
    ALIASES
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, c)| *c)
        .or_else(|| CODICONS.binary_search_by(|(n, _)| (*n).cmp(name)).ok().map(|at| CODICONS[at].1))
}

/// Whether the font has a codicon of this name.
// The parts to come ask; the window's first parts name only icons the font has (a test says so).
#[allow(dead_code)]
pub fn known(name: &str) -> bool {
    code_of(name).is_some()
}

/// A codicon's character, by its name in VS Code: any of the font's (`icons_table.rs`), "info" for a name it does
/// not have.
pub fn char_of(name: &str) -> char {
    char::from_u32(code_of(name).unwrap_or(UNKNOWN)).unwrap_or('?')
}

/// A codicon at VS Code's size for it (16 px, 24 in the activity bar).
pub fn icon(name: &'static str, size: f32) -> Label {
    let c = char_of(name).to_string();
    label(move || c.clone()).style(move |s| {
        s.font_family("codicon".to_string()).font_size(size).line_height(1.0).width(size).height(size).justify_center()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_codicon_is_found_by_its_name() {
        assert!(CODICONS.windows(2).all(|w| w[0].0 < w[1].0), "the names in order, each once");
        assert!(CODICONS.len() > 600);
        for (name, code) in CODICONS {
            // A name the window began with under another code point of the same glyph keeps the one it had.
            let want = ALIASES.iter().find(|(alias, _)| *alias == name).map_or(code, |(_, kept)| *kept);
            assert_eq!(char_of(name) as u32, want, "{name}");
        }
        assert!(known("source-control") && known("debug-alt") && known("extensions"));
        assert!(!known("no-such-icon"));
        assert_eq!(char_of("no-such-icon") as u32, UNKNOWN);
        // VS Code's other names are characters the font has.
        let has = |code: u32| CODICONS.iter().any(|(_, c)| *c == code);
        for (name, code) in ALIASES {
            assert!(has(code) || matches!(code, 0xec6f | 0xeb8d), "{name}");
        }
    }

    #[test]
    fn the_names_the_window_began_with_are_the_characters_they_were() {
        let first = [
            ("files", 0xeaf0),
            ("search", 0xea6d),
            ("source-control", 0xea68),
            ("git-compare", 0xeafd),
            ("symbol-class", 0xeb5b),
            ("comment-discussion", 0xeac7),
            ("terminal", 0xea85),
            ("account", 0xeb99),
            ("settings-gear", 0xeb51),
            ("chevron-right", 0xeab6),
            ("chevron-down", 0xeab4),
            ("chevron-up", 0xeab7),
            ("close", 0xea76),
            ("ellipsis", 0xea7c),
            ("split-horizontal", 0xeb56),
            ("layout-sidebar-left", 0xebf3),
            ("layout-sidebar-left-off", 0xec02),
            ("layout-panel", 0xebf2),
            ("layout-panel-off", 0xec01),
            ("layout-sidebar-right", 0xebf4),
            ("layout-sidebar-right-off", 0xec00),
            ("layout", 0xebeb),
            ("git-branch", 0xec6f),
            ("error", 0xea87),
            ("warning", 0xea6c),
            ("bell", 0xeaa2),
            ("check", 0xeab2),
            ("sync", 0xea77),
            ("send", 0xec0f),
            ("debug-stop", 0xead7),
            ("new-file", 0xea7f),
            ("new-folder", 0xea80),
            ("refresh", 0xeb37),
            ("collapse-all", 0xeac5),
            ("add", 0xea60),
            ("trash", 0xea81),
            ("sparkle", 0xec10),
            ("chrome-minimize", 0xeaba),
            ("chrome-maximize", 0xeab9),
            ("chrome-restore", 0xeabb),
            ("chrome-close", 0xeab8),
            ("remote", 0xeb3a),
            ("arrow-left", 0xea9b),
            ("arrow-right", 0xea9c),
            ("history", 0xea82),
            ("pass", 0xeba4),
            ("loading", 0xeb19),
            ("tools", 0xeb6d),
            ("attach", 0xec34),
            ("circle-filled", 0xea71),
            ("circle-large-filled", 0xebb4),
            ("file", 0xea7b),
            ("screen-full", 0xeb4c),
            ("case-sensitive", 0xeab1),
            ("regex", 0xeb38),
            ("arrow-up", 0xeaa1),
            ("arrow-down", 0xea9a),
            ("symbol-method", 0xea8c),
            ("symbol-struct", 0xea91),
            ("symbol-enum", 0xea95),
            ("symbol-interface", 0xeb61),
            ("symbol-namespace", 0xea8b),
            ("symbol-constant", 0xeb5d),
            ("symbol-variable", 0xea88),
            ("symbol-string", 0xeb8d),
            ("symbol-field", 0xeb5f),
        ];
        for (name, code) in first {
            assert_eq!(char_of(name) as u32, code, "{name}");
        }
    }
}
