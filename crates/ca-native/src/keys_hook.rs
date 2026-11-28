//! Which keys pressed where another part has the keys are the workbench's all the same: the terminal sends what it
//! is given to its shell, and a box to type in has a keymap of its own, so each asks here before it keeps a key.
//! Both ask the keymap (`keymap/mod.rs`) what the key is bound to where it was pressed.
//!
//! - `terminal(ui, key, mods)`: true when the workbench gets a key pressed in the terminal, not the shell: a key
//!   bound to one of the commands VS Code skips the shell for (`SKIP_SHELL`, with
//!   `terminal.integrated.commandsToSkipShell`: an id more, `-id` one less), a key that begins a chord or ends one
//!   begun (`terminal.integrated.allowChords`). With `terminal.integrated.sendKeybindingsToShell` only the
//!   commands that setting names are skipped for.
//! - `editor(ui, key, mods)`: true when the workbench is given a key a box's own keymap did not take: one bound to
//!   a command there now, one that begins a chord, and any key while a chord waits for its second.

use floem::keyboard::{Key, Modifiers};

use crate::settings;
use crate::state::Ui;

/// The commands whose keys the terminal leaves to the workbench: the views, the panel and the side bars, the
/// palette and Quick Open, the editors' and the groups' moves, the terminal's and the debugger's own. Saving,
/// zooming, quitting and the like stay the shell's (Ctrl+S, Ctrl+-, Ctrl+Q are a shell's keys too) unless
/// `terminal.integrated.commandsToSkipShell` names them.
pub const SKIP_SHELL: &[&str] = &[
    "workbench.action.showCommands",
    "workbench.action.quickOpen",
    "workbench.action.quickOpenView",
    "workbench.action.toggleSidebarVisibility",
    "workbench.action.toggleAuxiliaryBar",
    "workbench.action.togglePanel",
    "workbench.action.toggleMaximizedPanel",
    "workbench.action.closeActiveEditor",
    "workbench.action.nextEditor",
    "workbench.action.previousEditor",
    "workbench.action.focusActiveEditorGroup",
    "workbench.action.focusFirstEditorGroup",
    "workbench.action.focusSecondEditorGroup",
    "workbench.action.focusThirdEditorGroup",
    "workbench.action.navigateBack",
    "workbench.action.navigateForward",
    "workbench.action.selectTheme",
    "workbench.action.chat.open",
    "workbench.view.explorer",
    "workbench.view.search",
    "workbench.view.scm",
    "workbench.view.debug",
    "workbench.view.extensions",
    "workbench.actions.view.problems",
    "workbench.action.findInFiles",
    "workbench.action.output.toggleOutput",
    "workbench.debug.action.toggleRepl",
    "workbench.action.debug.start",
    "workbench.action.debug.run",
    "workbench.action.debug.continue",
    "workbench.action.debug.stop",
    "workbench.action.debug.restart",
    "workbench.action.debug.stepOver",
    "workbench.action.debug.stepInto",
    "workbench.action.debug.stepOut",
    "workbench.action.tasks.build",
    "workbench.action.terminal.toggleTerminal",
    "workbench.action.terminal.focus",
    "workbench.action.terminal.new",
    "workbench.action.terminal.kill",
    "workbench.action.terminal.split",
    "workbench.action.terminal.focusNext",
    "workbench.action.terminal.focusPrevious",
    "workbench.action.terminal.copySelection",
    "workbench.action.terminal.paste",
    "workbench.action.terminal.scrollUp",
    "workbench.action.terminal.scrollDown",
    "workbench.action.terminal.scrollToTop",
    "workbench.action.terminal.scrollToBottom",
];

/// Whether the terminal leaves a command's keys to the workbench: the default list, with the setting's ids added
/// and its `-id`s taken away; only the setting's ids when every key is to reach the shell.
pub fn skips_shell(id: &str, setting: &[String], all_to_shell: bool) -> bool {
    if setting.iter().any(|s| s.strip_prefix('-') == Some(id)) {
        return false;
    }
    setting.iter().any(|s| s == id) || (!all_to_shell && SKIP_SHELL.contains(&id))
}

/// Whether a key pressed in the terminal is the workbench's, not the shell's.
pub fn terminal(ui: Ui, key: &Key, m: Modifiers) -> bool {
    let chords = settings::now(ui, "terminal.integrated.allowChords", None).as_bool().unwrap_or(true);
    let all_to_shell = settings::now(ui, "terminal.integrated.sendKeybindingsToShell", None).as_bool().unwrap_or(false);
    if crate::keymap::waiting().is_some() {
        return chords;
    }
    if crate::keymap::begins_chord(ui, key, m) {
        return chords && !all_to_shell;
    }
    let Some(id) = crate::keymap::command_of(ui, key, m) else { return false };
    let setting: Vec<String> = settings::now(ui, "terminal.integrated.commandsToSkipShell", None)
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    skips_shell(&id, &setting, all_to_shell)
}

/// Whether a key a box's own keymap did not take goes to the workbench.
pub fn editor(ui: Ui, key: &Key, m: Modifiers) -> bool {
    crate::keymap::waiting().is_some() || crate::keymap::bound(ui, key, m)
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use floem::keyboard::NamedKey;

    use super::*;
    use crate::commands::{add, command};
    use crate::state::Send;

    fn k(c: &str) -> Key {
        Key::Character(c.into())
    }

    #[test]
    fn the_workbenchs_keys_are_not_the_shells_nor_the_boxes() {
        let ui = Ui::new();
        let send: Send = Rc::new(|_| {});
        crate::workbench::register(ui, &send);
        add(command("test.hook.mine", "", "Mine", |_, _| {}).keys("Ctrl+Alt+F9"));
        add(command("test.hook.chord", "", "Chord", |_, _| {}).keys("Ctrl+Alt+K Ctrl+Alt+F8"));
        let ctrl = Modifiers::CONTROL;
        for c in ["b", "J", "`", "p", "W"] {
            assert!(terminal(ui, &k(c), ctrl), "{c}");
        }
        assert!(terminal(ui, &k("p"), ctrl | Modifiers::SHIFT));
        assert!(terminal(ui, &Key::Named(NamedKey::F1), Modifiers::empty()));
        // The shell's own: Ctrl+C, Ctrl+R, a letter typed, a key bound to a command not on the list.
        assert!(!terminal(ui, &k("c"), ctrl));
        assert!(!terminal(ui, &k("r"), ctrl));
        assert!(!terminal(ui, &k("b"), Modifiers::empty()));
        assert!(!terminal(ui, &Key::Named(NamedKey::F9), ctrl | Modifiers::ALT));
        // A chord begins in the terminal, and its second key is the workbench's.
        assert!(terminal(ui, &k("k"), ctrl | Modifiers::ALT));
        // A box gives the workbench what is bound, not what is typed.
        assert!(editor(ui, &k("b"), ctrl));
        assert!(editor(ui, &Key::Named(NamedKey::F1), Modifiers::empty()));
        assert!(editor(ui, &Key::Named(NamedKey::F9), ctrl | Modifiers::ALT));
        assert!(!editor(ui, &k("b"), Modifiers::SHIFT));
        assert!(!editor(ui, &k("x"), Modifiers::empty()));
    }

    #[test]
    fn the_setting_adds_to_the_commands_that_skip_the_shell_and_takes_from_them() {
        let none: Vec<String> = Vec::new();
        assert!(skips_shell("workbench.action.quickOpen", &none, false));
        assert!(!skips_shell("workbench.action.quickOpen", &none, true));
        assert!(!skips_shell("mine", &none, false));
        let setting = vec!["mine".to_string(), "-workbench.action.quickOpen".to_string()];
        assert!(skips_shell("mine", &setting, false));
        assert!(skips_shell("mine", &setting, true));
        assert!(!skips_shell("workbench.action.quickOpen", &setting, false));
        assert!(skips_shell("workbench.action.togglePanel", &setting, false));
    }
}
