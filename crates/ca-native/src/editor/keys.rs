//! The editor's keys: its own keymap's, and the workbench's that are about the editor.

use floem::keyboard::{Key, Modifiers, NamedKey};
use floem::reactive::{RwSignal, SignalGet, SignalWith};
use floem::views::editor::Editor;
use floem::views::editor::command::CommandExecuted;
use floem::views::editor::core::mode::Mode;
use floem::views::editor::keypress::KeypressMap;
use floem::views::editor::keypress::key::KeyInput;
use floem::views::editor::keypress::press::KeyPress;

use super::find::find_show;
use super::tell;
use crate::state::Ui;

/// A key as the editor's own keymap has it (what `default_key_handler` does, with the keymap made once).
pub(super) fn default_key(
    map: &KeypressMap,
    editor: RwSignal<Editor>,
    press: &KeyPress,
    mods: Modifiers,
) -> CommandExecuted {
    let command = map.keymaps.get(press).or_else(|| {
        let insert = editor.with_untracked(|e| e.cursor.with_untracked(|c| c.get_mode())) == Mode::Insert;
        let mut press = press.clone();
        press.mods.set(Modifiers::SHIFT, false);
        if insert { map.keymaps.get(&press) } else { None }
    });
    match command {
        Some(command) => editor.with_untracked(|e| e.doc().run_command(e, command, Some(1), mods)),
        None => CommandExecuted::No,
    }
}

/// The workbench's keys that are about the editor, taken by the editor itself (a key it is given goes no further):
/// the suggest widget's (`complete::key`), the find widget's (`find::find_key`: Ctrl+H, F3 and Shift+F3, its toggles, Escape), Ctrl+F finds, Ctrl+S saves,
/// Ctrl+W closes the tab.
pub(super) fn editor_key(ui: Ui, press: &KeyPress, mods: Modifiers) -> bool {
    let KeyInput::Keyboard(key, _) = &press.key else { return false };
    // The suggest widget's and the ghost text's keys first (Enter, Tab, Escape, the arrows while it is open).
    if crate::complete::key(ui, key, mods) {
        return true;
    }
    if super::find::find_key(ui, key, mods) {
        return true;
    }
    // The editing commands' keys (`editing.rs`), and Tab, Enter and Backspace as VS Code's.
    if super::editing::key(ui, press, mods) {
        return true;
    }
    let shift = mods.contains(Modifiers::SHIFT);
    let ctrl = mods.contains(Modifiers::CONTROL) && !mods.contains(Modifiers::ALT);
    match key {
        Key::Character(c) if ctrl && !shift => match c.to_lowercase().as_str() {
            "f" => find_show(ui),
            "s" => tell(ui, |send| ui.save(send)),
            "w" => {
                if let Some(p) = ui.active.get_untracked() {
                    ui.close(&p);
                }
            }
            _ => return false,
        },
        // Ctrl with Page Up or Page Down is the workbench's (the next or previous tab, a tab moved), as in VS
        // Code: Floem's editor would take it as a page scroll.
        Key::Named(NamedKey::PageDown | NamedKey::PageUp) if mods.contains(Modifiers::CONTROL) => {
            tell(ui, |send| crate::workbench::key(ui, send, key, mods));
        }
        _ => return false,
    }
    true
}

/// What it adds to the window's registries, once at start.
pub fn register(_ui: crate::state::Ui, _send: &crate::state::Send) {}

/// The session's messages that are this part's, taken in; true takes one away from the parts after it.
pub fn take(_ui: crate::state::Ui, _m: &serde_json::Value, _send: &crate::state::Send) -> bool {
    false
}
