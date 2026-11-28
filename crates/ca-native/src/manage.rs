//! The Manage menu of the activity bar's gear, as VS Code's: the Command Palette and the colour theme (which the
//! activity bar adds), and here Settings, Extensions and Keyboard Shortcuts, each with its keys. A row whose command
//! no part has added yet is dimmed; Settings runs Open Settings (Ctrl+,), which opens the settings.json until a part
//! brings the Settings editor under the same command id. The parts with more to say add their rows to
//! `Menu::Manage` under these groups: `1_commands`, `2_configuration`, `5_themes`, `6_notifications`
//! (Desktop Notifications), `7_tools` (the model's tool mode), `8_feedback` (Send Feedback…), `9_about`.

use serde_json::Value;

use crate::state::{Send, Ui};

/// VS Code's Settings editor.
pub const OPEN_SETTINGS: &str = "workbench.action.openSettings";
/// VS Code's Keyboard Shortcuts editor.
pub const OPEN_KEYBINDINGS: &str = "workbench.action.openGlobalKeybindings";
/// VS Code's Extensions view.
pub const SHOW_EXTENSIONS: &str = "workbench.view.extensions";

/// The gear's rows of this part.
pub fn register(_ui: Ui, _send: &Send) {
    use crate::menus::{Menu, add, item};
    let group = "2_configuration";
    // Open Settings: the settings.json until the Settings editor takes the command's place.
    add(Menu::Manage, item(OPEN_SETTINGS).title("Settings").group(group).order(1));
    add(Menu::Manage, item(SHOW_EXTENSIONS).title("Extensions").group(group).order(2));
    add(
        Menu::Manage,
        item(OPEN_KEYBINDINGS).title("Keyboard Shortcuts").shown_keys("Ctrl+K Ctrl+S").group(group).order(3),
    );
}

/// Nothing to take.
pub fn take(_ui: Ui, _m: &Value, _send: &Send) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::*;
    use crate::menus::{Menu, Row, add, item, rows};

    fn shown(ui: Ui) -> Vec<String> {
        rows(ui, Menu::Manage)
            .into_iter()
            .map(|r| match r {
                Row::Line => "-".to_string(),
                Row::Entry { title, keys, command, .. } => {
                    let dim = if command.is_some() { "" } else { " (dimmed)" };
                    format!("{title}{}{keys}{dim}", if keys.is_empty() { "" } else { " " })
                }
            })
            .collect()
    }

    #[test]
    fn the_gear_has_the_palette_then_settings_extensions_and_keys_then_the_theme() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        crate::workbench::register(ui, &send);
        crate::settings::register(ui, &send);
        crate::settings::editor::register(ui, &send);
        crate::activity_bar::register(ui, &send);
        register(ui, &send);
        let rows = shown(ui);
        let at = |t: &str| rows.iter().position(|r| r.starts_with(t)).unwrap_or_else(|| panic!("{t} in {rows:?}"));
        assert!(at("Command Palette...") < at("Settings"));
        assert!(at("Settings") < at("Keyboard Shortcuts") && at("Keyboard Shortcuts") < at("Themes: Color Theme"));
        // Open Settings is there (the Settings editor), with VS Code's Ctrl+,; the Keyboard Shortcuts editor
        // is not there yet.
        assert_eq!(rows.iter().filter(|r| r.starts_with("Settings")).count(), 1);
        assert_eq!(rows[at("Settings")], format!("Settings {}", crate::commands::keys_of(OPEN_SETTINGS)));
        assert!(!crate::commands::keys_of(OPEN_SETTINGS).is_empty());
        assert!(crate::commands::run(ui, &send, OPEN_SETTINGS));
        assert_eq!(rows[at("Keyboard Shortcuts")], "Keyboard Shortcuts Ctrl+K Ctrl+S (dimmed)");
        // Once a part brings the Settings editor, its row takes the place of the settings.json's.
        crate::commands::add(crate::commands::command(OPEN_SETTINGS, "Preferences", "Open Settings", |_, _| {}));
        add(Menu::Manage, item(OPEN_SETTINGS).title("Settings").group("2_configuration").order(1));
        let rows = shown(ui);
        // VS Code's default table gives it its key.
        assert_eq!(
            rows.iter().filter(|r| r.starts_with("Settings")).collect::<Vec<_>>(),
            [&format!("Settings {}", crate::commands::keys_of(OPEN_SETTINGS))]
        );
        assert!(!crate::commands::keys_of(OPEN_SETTINGS).is_empty());
    }
}
