//! The Manage menu of the activity bar's gear, as VS Code's: the Command Palette and the colour theme (which the
//! activity bar adds), and here Settings, Extensions and Keyboard Shortcuts, each with its keys. A row whose command
//! no part has added yet is dimmed; Settings runs Open Settings (Ctrl+,), which opens the settings.json until a part
//! brings the Settings editor under the same command id. The parts with more to say add their rows to
//! `Menu::Manage` under these groups: `1_commands`, `2_configuration`, `5_themes`, `6_notifications`
//! (Desktop Notifications), `7_tools` (the model's tool mode), `8_feedback` (Send Feedback…), `9_about`.
//!
//! The model's tool mode is here: under "<model>'s tools", As Arena says (its word for the model), Direct, As code
//! and Both, the one in force checked. A choice is kept in the person's settings (`tools.mode`) by the driver.

use floem::reactive::{RwSignal, SignalGet, SignalUpdate, SignalWith};
use serde_json::{Value, json};

use crate::state::{Send, Ui};

/// VS Code's Settings editor.
pub const OPEN_SETTINGS: &str = "workbench.action.openSettings";
/// VS Code's Keyboard Shortcuts editor.
pub const OPEN_KEYBINDINGS: &str = "workbench.action.openGlobalKeybindings";
/// VS Code's Extensions view.
pub const SHOW_EXTENSIONS: &str = "workbench.view.extensions";

/// Follow Arena's word for the model's tools.
pub const TOOLS_AS_ARENA_SAYS: &str = "codeArena.toolMode.arena";
/// The model's tools one call at a time.
pub const TOOLS_DIRECT: &str = "codeArena.toolMode.direct";
/// The servers' tools as functions of a run_code program.
pub const TOOLS_AS_CODE: &str = "codeArena.toolMode.code";
/// Every tool directly, and run_code too.
pub const TOOLS_BOTH: &str = "codeArena.toolMode.both";

/// How the model is given its tools, as the driver says it (`state.tool_mode`, `tool_mode`).
#[derive(Clone, Copy)]
pub struct ToolMode {
    /// The mode in force: direct, code or both.
    pub value: RwSignal<String>,
    /// The person chose it; else it follows Arena's word.
    pub set: RwSignal<bool>,
    /// Arena's word for the model, when it has one.
    pub arena: RwSignal<Option<String>>,
}

impl crate::ext::State for ToolMode {
    fn new(ui: Ui) -> Self {
        ToolMode {
            value: ui.scope.create_rw_signal("both".into()),
            set: ui.scope.create_rw_signal(false),
            arena: ui.scope.create_rw_signal(None),
        }
    }
}

impl ToolMode {
    /// Takes in what the driver said, setting only what changed.
    fn take(&self, v: &Value) {
        if let Some(value) = v["value"].as_str()
            && self.value.with_untracked(|now| now != value)
        {
            self.value.set(value.to_string());
        }
        let set = v["set"] == true;
        if self.set.get_untracked() != set {
            self.set.set(set);
        }
        let arena = v["arena"].as_str().map(str::to_string);
        if self.arena.with_untracked(|now| *now != arena) {
            self.arena.set(arena);
        }
    }

    /// The row checked: `arena` while it follows Arena's word, else the mode chosen.
    fn checked(&self) -> String {
        if self.set.get() { self.value.get() } else { "arena".into() }
    }
}

/// A mode's words in the menu.
fn mode_title(mode: &str) -> &'static str {
    match mode {
        "direct" => "Direct",
        "code" => "As code",
        _ => "Both",
    }
}

/// The gear's rows of this part.
pub fn register(ui: Ui, _send: &Send) {
    use crate::commands::command;
    use crate::menus::{Menu, add, item};
    // The model's tools: the commands (in the palette too) and their rows under the model's name.
    for (id, mode, title) in [
        (TOOLS_AS_ARENA_SAYS, None, "Model's Tools: As Arena Says"),
        (TOOLS_DIRECT, Some("direct"), "Model's Tools: Direct"),
        (TOOLS_AS_CODE, Some("code"), "Model's Tools: As Code"),
        (TOOLS_BOTH, Some("both"), "Model's Tools: Both"),
    ] {
        crate::commands::add(command(id, "Code Arena", title, move |_, send| {
            send(json!({"type": "tool_mode", "value": mode, "key": mode.unwrap_or("arena")}));
        }));
    }
    let _ = ui.state::<ToolMode>();
    add(
        Menu::Manage,
        crate::menus::soon("The model's tools", "").group("7_tools").order(0).live_title(|ui| {
            let model = ui.model.get_untracked();
            if model.is_empty() { "The model's tools".into() } else { format!("{model}'s tools") }
        }),
    );
    add(
        Menu::Manage,
        item(TOOLS_AS_ARENA_SAYS)
            .title("As Arena says")
            .group("7_tools")
            .order(1)
            .live_title(|ui| {
                let arena = ui.state::<ToolMode>().arena.get_untracked();
                format!("As Arena says: {}", mode_title(arena.as_deref().unwrap_or("both")).to_lowercase())
            })
            .checked(|ui| ui.state::<ToolMode>().checked() == "arena"),
    );
    for (order, (id, mode)) in
        [(TOOLS_DIRECT, "direct"), (TOOLS_AS_CODE, "code"), (TOOLS_BOTH, "both")].into_iter().enumerate()
    {
        add(
            Menu::Manage,
            item(id)
                .title(mode_title(mode))
                .group("7_tools")
                .order(order as i32 + 2)
                .checked(move |ui| ui.state::<ToolMode>().checked() == mode),
        );
    }
    let group = "2_configuration";
    // Open Settings: the settings.json until the Settings editor takes the command's place.
    add(Menu::Manage, item(OPEN_SETTINGS).title("Settings").group(group).order(1));
    add(Menu::Manage, item(SHOW_EXTENSIONS).title("Extensions").group(group).order(2));
    add(
        Menu::Manage,
        item(OPEN_KEYBINDINGS).title("Keyboard Shortcuts").shown_keys("Ctrl+K Ctrl+S").group(group).order(3),
    );
}

/// How the model is given its tools, from the session's state and the driver's answer to a choice.
pub fn take(ui: Ui, m: &Value, _send: &Send) -> bool {
    match m["type"].as_str() {
        Some("state") if m["tool_mode"].is_object() => ui.state::<ToolMode>().take(&m["tool_mode"]),
        Some("tool_mode") => {
            ui.state::<ToolMode>().take(m);
            return true;
        }
        _ => {}
    }
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
    fn the_models_tools_are_under_its_name_the_one_in_force_checked_and_a_choice_is_sent() {
        let ui = Ui::new();
        let sent = Rc::new(std::cell::RefCell::new(Vec::new()));
        let to = sent.clone();
        let send: Send = Rc::new(move |v| to.borrow_mut().push(v));
        register(ui, &send);
        ui.model.set("qwen".into());
        let tools = |ui: Ui| -> Vec<String> {
            let rows = shown(ui);
            let at = rows.iter().position(|r| r.starts_with("qwen's tools")).expect("the header");
            rows[at..at + 5].to_vec()
        };
        let checked = |ui: Ui| -> Vec<String> {
            rows(ui, Menu::Manage)
                .into_iter()
                .filter_map(|r| match r {
                    Row::Entry { title, checked: true, .. } => Some(title),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(tools(ui), ["qwen's tools (dimmed)", "As Arena says: both", "Direct", "As code", "Both"]);
        assert_eq!(checked(ui), ["As Arena says: both"]);
        // Arena's word is said in its row; the person's choice is checked in place of it.
        take(ui, &json!({"type": "state", "tool_mode": {"value": "direct", "set": false, "arena": "direct"}}), &send);
        assert_eq!(checked(ui), ["As Arena says: direct"]);
        assert!(take(
            ui,
            &json!({"type": "tool_mode", "value": "code", "set": true, "arena": "direct", "key": "code"}),
            &send
        ));
        assert_eq!(checked(ui), ["As code"]);
        assert!(crate::commands::run(ui, &send, TOOLS_BOTH));
        assert!(crate::commands::run(ui, &send, TOOLS_AS_ARENA_SAYS));
        assert_eq!(
            *sent.borrow(),
            [
                json!({"type": "tool_mode", "value": "both", "key": "both"}),
                json!({"type": "tool_mode", "value": null, "key": "arena"})
            ]
        );
    }

    #[test]
    fn the_gear_has_the_palette_then_settings_extensions_and_keys_then_the_theme() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        crate::workbench::register(ui, &send);
        crate::settings::register(ui, &send);
        crate::activity_bar::register(ui, &send);
        register(ui, &send);
        let rows = shown(ui);
        let at = |t: &str| rows.iter().position(|r| r.starts_with(t)).unwrap_or_else(|| panic!("{t} in {rows:?}"));
        assert!(at("Command Palette...") < at("Settings"));
        assert!(at("Settings") < at("Keyboard Shortcuts") && at("Keyboard Shortcuts") < at("Themes: Color Theme"));
        // Open Settings is there (the settings.json for now), with VS Code's Ctrl+,; the Keyboard Shortcuts editor
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
